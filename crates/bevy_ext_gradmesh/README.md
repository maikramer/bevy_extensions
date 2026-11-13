# bevy_ext_gradmesh

Procedural rendering for the Vael's authored primitives, on top of
[`bevy_gradmesh`](https://crates.io/crates/bevy_gradmesh).

The **contract** (`Shape`, `MaterialSpec`, `Preset`, the DSL attribute parser,
bounds and the `PrimitivePart` marker) lives in
[`bevy_ext_scene::primitives`](../bevy_ext_scene/src/primitives). This crate is
the render half and is used by both the game and the Jackdaw viewport preview,
so they cannot disagree about the look.

- `gradmesh_shape` / `build_mesh` — watertight `bevy_gradmesh` geometry with a
  subtle per-object vertical gradient baked into vertex colours (shared
  materials, no per-entity uniforms).
- `style` — colour/PBR plus the procedural look presets that replace the
  sampled albedo/normal maps.
- `collider` (feature `physics`) — exact Rapier colliders: analytic for the
  simple primitives, a trimesh for lathes/sweeps/extruded polygons/pyramids/
  tori.

## Example

```rust
use bevy_ext_gradmesh::{
    MaterialSpec, Preset, Shape, ShapeDetail, build_mesh, collider, style,
};

let barrel = Shape::Lathe {
    profile: vec![[0.42, -0.55], [0.58, 0.0], [0.42, 0.55]],
    segments: 24,
};
let material = MaterialSpec {
    base_color: Some([0.48, 0.36, 0.23]),
    preset: Some(Preset::Wood),
    ..Default::default()
};

let mesh = build_mesh(&barrel, ShapeDetail::High).expect("lathe geometry");
let gradient = style(&material).build();
let hitbox = collider(&barrel, Vec3::ONE).expect("exact collider");
```

## Integration

- Game: `VaelPrimitivesPlugin` (`../Vael/src/primitives.rs`) consumes the
  engine's `PrimitivePart` bridge and swaps the mesh/material/collider. The
  plugin enables the `physics` feature and registers `GradMeshPlugin`.
- Editor: `vael_jackdaw` reads `WorldNode` attributes and spawns editor-hidden
  preview children with the same conversion.
- The transitional Viber engine only parses the DSL and leaves a
  `StandardMaterial` fallback plus the marker; it does not depend on this
  crate.
