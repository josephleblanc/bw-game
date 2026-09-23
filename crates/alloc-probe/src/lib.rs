//! `#[alloc_probe]` — per-function allocation attribution (ADR 0004, D4).
//!
//! The attribute expands a function to the same function with a probe guard
//! inserted as its first statement:
//!
//! ```ignore
//! #[alloc_probe]
//! fn step_sim(/* … */) {
//!     // becomes the first statement:
//!     #[cfg(feature = "perf-alloc")]
//!     let _probe = ::bw_core::alloc_probe::Probe::new("step_sim");
//!     // …original body…
//! }
//! ```
//!
//! The guard snapshots dhat's monotonic `HeapStats` totals at entry and
//! accumulates the delta into the probe registry at drop — totals, never
//! current bytes, so allocations made-and-freed inside the function are
//! counted (ADR 0004, D5). The runtime lives in `bw_core::alloc_probe`
//! behind the `perf-alloc` feature; the emitted `cfg` resolves in the
//! *using* crate, so probes compile to nothing in timing and shipped
//! builds (ADR 0001, D8.8) and the counting allocator never perturbs a
//! timing run. Using crates must depend on `bw-core` and declare a
//! `perf-alloc` feature that enables `bw-core/perf-alloc`.
//!
//! Hand-written, dependency-free token surgery: the shape this macro
//! replaces was hand-proven first (`crates/demo`, commits 05cbf79…8b959b5)
//! and is the spec. Known limitation: signature positions before the body
//! must not contain brace groups (const generics); none do in this
//! workspace, and a violation fails loudly rather than mis-probing.

use proc_macro::{Delimiter, Group, TokenStream, TokenTree};

#[proc_macro_attribute]
pub fn alloc_probe(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return compile_error("#[alloc_probe] takes no arguments");
    }
    expand(item)
}

fn expand(item: TokenStream) -> TokenStream {
    let tokens: Vec<TokenTree> = item.into_iter().collect();
    let Some(fn_at) = tokens.iter().position(|tt| match tt {
        TokenTree::Ident(id) => id.to_string() == "fn",
        _ => false,
    }) else {
        return compile_error("#[alloc_probe] requires a function item");
    };
    let Some(TokenTree::Ident(name)) = tokens.get(fn_at + 1) else {
        return compile_error("#[alloc_probe]: expected a function name after `fn`");
    };
    // The first brace group after `fn` is the body. Signature types, where
    // clauses, and attributes use parens/angles/brackets, never braces
    // (no const generics — see the crate docs).
    let Some(body_at) = tokens
        .iter()
        .enumerate()
        .skip(fn_at + 1)
        .find_map(|(i, tt)| match tt {
            TokenTree::Group(g) if g.delimiter() == Delimiter::Brace => Some(i),
            _ => None,
        })
    else {
        return compile_error("#[alloc_probe]: function has no body to probe");
    };
    let TokenTree::Group(body) = &tokens[body_at] else {
        return compile_error("#[alloc_probe]: internal error: body position not a group");
    };

    let mut new_body = probe_statement(&name.to_string());
    new_body.extend(body.stream());
    let mut probed = Group::new(Delimiter::Brace, new_body);
    probed.set_span(body.span());

    let mut out = tokens[..body_at].to_vec();
    out.push(TokenTree::Group(probed));
    out.extend(tokens[body_at + 1..].iter().cloned());
    out.into_iter().collect()
}

/// The guard statement the macro inserts, as tokens. Built from a string
/// interpolated with the function name only (a lexed identifier), so lexing
/// cannot fail in practice; the fallback keeps that true rather than hoped.
fn probe_statement(name: &str) -> TokenStream {
    let text = format!(
        "#[cfg(feature = \"perf-alloc\")] \
         let _probe = ::bw_core::alloc_probe::Probe::new(\"{name}\");"
    );
    text.parse().unwrap_or_else(|_| {
        compile_error(&format!(
            "#[alloc_probe]: failed to build the probe statement for `{name}`"
        ))
    })
}

fn compile_error(message: &str) -> TokenStream {
    format!("compile_error!({message:?});")
        .parse()
        .unwrap_or_else(|_| TokenStream::new())
}
