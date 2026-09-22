use bevy::prelude::*;

fn main() {
    // Phase-1 smoke app: proves the minimal Bevy dependency builds and runs
    // headless. The real gallery lands with Phase 2 (ADR 0001).
    let mut app = App::new();
    app.add_systems(Update, tick);
    for _ in 0..3 {
        app.update();
    }
    println!("bevy minimal app: 3 updates ok");
}

fn tick(mut count: Local<u32>) {
    *count += 1;
}
