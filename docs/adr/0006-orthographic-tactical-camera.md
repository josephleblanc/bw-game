# ADR 0006: Adopt the orthographic tactical camera

- Status: Accepted
- Date: 2026-09-23

## Context

The walker playground's camera has converged through three stages: an
over-the-shoulder chase (illegible gaits down the stride axis), a follow
camera pinned to the SVG renderer's establishing-shot orientation
(perspective), and — after hands-on play — the user's preference for the
orthographic variant of that view. The game direction is an ACS-like
colony sim: many small figures on a tiled ground, read as a whole. The
same lesson the isometric/tactics genre learned long ago applies here:
orthographic projection makes a tactical field legible — sizes and tiles
stay comparable across the field, distances read directly, and crowded
scenes don't collapse into a perspective wedge.

Two failure modes surfaced along the way that the projection alone does
not fix, and this ADR records their answers too:

- At exactly 45° azimuth the checkerboard reads as a perfectly regular
  wallpaper — both families of tile edges mirror each other and the
  ground becomes noise.
- A dead-rear view (chase camera, or any view straight down an action
  axis) foreshortens gaits and strikes to nothing.

## Decision

1. **The orthographic camera is the default tactical presentation** for
   live viewers in this project. The walker playground opens
   orthographic; `O` toggles a perspective view of the same orientation
   (kept for depth cues and preference checks).
2. **The tactical orientation is 49° azimuth (off +z toward +x) at
   ~15.5° elevation**, unit-pinned by test. The 4° offset off the 45°
   diagonal breaks the tile wallpaper (one family of edges dominates,
   giving the view a directional hierarchy — the same trick
   isometric-style games use when they offset the classic angle); the
   shallow elevation keeps figures' sagittal plane readable at crowd
   scale.
3. **Orthographic zoom rides the projection's `scale`**, with
   `ScalingMode::FixedVertical` sizing the view (~8 m, matching the
   perspective framing at the follow distance). `-/+` therefore behave
   identically in both projections; the follow distance is a no-op for
   ortho framing (it only matters for near/far and the perspective
   toggle).
4. **Picking is projection-agnostic by construction**: the cursor ray
   comes from `Camera::viewport_to_world`, so ground sends and figure
   selection work unchanged under either projection.

## Alternatives considered

- **Perspective as default** (the pre-ADR state): kept as the toggle,
  not the default — the tactical read won hands-down in play.
- **True isometric (30°/35.26°) or 2:1 dimetric elevation**: the genre
  classics. Rejected for now — at our shallow ~15.5° elevation figures'
  gaits and facing stay legible in crowds, which is the playground's
  job; a steeper elevation can be tried by changing one constant plus
  its test.
- **Exact 45° azimuth**: rejected — the regular-tile wallpaper effect
  the offset exists to break.

## Consequences

- Offline SVG renders still use their perspective pinhole; live views
  and offline renders no longer match bit-for-bit in style. Acceptable:
  renders are previews of poses, not of presentation. Porting the SVG
  camera to orthographic (dropping the per-vertex depth divide) is a
  small future step this ADR sanctions if the divergence starts to
  matter.
- Anything that assumed perspective (fog, distance-based fading, depth
  cues) won't apply under the default view; don't build mechanics that
  depend on them.
- The orientation constants live in the viewer with a pinning test;
  when a second surface (a future map viewer) needs the same tactical
  view, promote them to a shared module rather than copying.

## Implementation notes

- `crates/walker-gallery/src/viewer.rs`: `CAM_AZIMUTH_DEG` /
  `CAM_ELEVATION_DEG`, `ortho_projection()` /
  `perspective_projection()`, `ortho_zoom`, the `O` toggle, and the
  `follow_camera_holds_the_tactical_orientation` pin. The viewer opens
  orthographic (`Viewer.ortho = true`, camera spawned with
  `ortho_projection()`).
- The `--viewer-shot` smoke runs its first seven shots under ortho and
  swaps to perspective for the final "persp" capture, so both
  projections stay verified.
