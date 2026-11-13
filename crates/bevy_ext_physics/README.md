# bevy_ext_physics

Rapier physics for the Viber world contract, extracted from the engine
monolith:

- `physics` — the `PhysicsPlugin`: Rapier wiring, the declarative
  `collider=`/`rigidbody=` parsing helpers (box/sphere/cylinder/capsule,
  trimesh/precompute/auto with a warn-once `ColliderCache`), deferred shapes
  (`PendingCollider` + `resolve_pending_colliders`), exact primitive colliders
  for composition parts (`collider_for_shape`), the debug-render toggle and
  the voxel terrain collision streaming (`stream_voxel_colliders`,
  `TerrainCollisionStatus`, per-column trimesh bake).
- `physics_fx` — transform-driven impact FX: `knockback_system` (exact
  exponential decay, world-border clamp, terrain settle) and `falling_system`
  (topple + despawn for `break-style: fall` destructibles). The frozen types
  (`Knockback`, `Falling`) and the pure math live in `bevy_ext_core`.
- `ragdoll` — short death ragdolls (`RagdollDeath`): a reduced bone chain
  becomes dynamic bodies with spherical joints for `RAGDOLL_SECS`, then the
  corpse fade continues from the last pose.

Reflected types keep the historical `viber::physics` / `viber::physics_fx` /
`viber::ragdoll` type paths so `.bsn` files, saves and BRP clients keep
working.

## Status

Transitional: terrain runtime, hero ground sampling and the animation/corpse
interop still come from the engine monolith (see `Cargo.toml`,
`TODO(viber-exit)`).

## Bevy version

Built against Bevy 0.20.1 and `bevy_rapier3d` 0.37 (same line as the engine).

## License

MIT OR Apache-2.0.
