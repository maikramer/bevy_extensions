# bevy_ext_puppet

Dynamic motion synthesis for humanoids on Bevy 0.20 + Avian 0.8, in the
spirit of NaturalMotion's Euphoria: the character gets a physical body whose
**muscles track the animation**, it **senses its balance**, and a network of
**behaviours** reacts to the world (balance, stagger, windmill, catch the
fall, writhe, get up).

```rust
app.add_plugins((PhysicsPlugins::default(), PuppetPlugin::default()));

// Any character whose skeleton uses UE-mannequin or Mixamo bone names:
commands.spawn((SceneRoot(hero_glb), Puppet));

commands.entity(hero).puppet_push(Vec3::Z * 50.0);                                       // shove
commands.entity(hero).puppet_bump(PuppetPart::UpperArmR, dir * 12.0, Some(bumper_pos));  // shoulder bump + glance
commands.entity(hero).puppet_hit_kind(PuppetPart::SpineLower, dir * 24.0, Some(p), HitKind::Blunt); // anatomical hit
commands.entity(hero).puppet_trip(Side::Right, fwd * 36.0);                              // trip
commands.entity(hero).puppet_look_at(threat_pos, 1.5);                                   // orienting gaze
commands.entity(hero).puppet_kill();                                                     // dying collapse → ragdoll
commands.entity(hero).puppet(PuppetCommand::Revive);                                     // physical 3-stage get-up
```

Run the sandbox (`1`–`5` push/wall shove, `B` bump, `H/C/U/L` anatomical hits, `T` trip, `J` high drop, `K` kill, `R` get up, `G` gizmos):

```text
cargo run -p bevy_ext_puppet --example puppet_sandbox --features sandbox --release
```

## How it works

```text
animation ──▶ PuppetTargets (animated pose, PostUpdate)
                 │
sense + neuro ─▶ behaviours ──▶ PuppetRequests: joint targets by priority,
                 │               arm/leg VirtualForce, gaze, pins, signals (physics step)
                 ▼  junction (highest priority wins, equal ones blend)
MuscleJoint (XPBD + Hill force–velocity + anisotropic swing/twist + passive viscosity)
                 │──▶ 16 rigid bodies + biased anatomical SphericalJoint / RevoluteJoint limits
                 ▼
PuppetBlend ──▶ bones (physics weight per part, PostUpdate)
```

| Module | What |
|---|---|
| `part` | the 16 parts, `BodyZone`, bone names (UE + Mixamo), de Leva mass fractions, asymmetric anatomical joint limits, anisotropic twist scales & passive damping coefficients |
| `rig` | builds bodies/joints from the skeleton (`Puppet` → `PuppetRig`) with exact Avian 0.8 `SphericalJoint` cone/twist alignment, `PuppetConfig` |
| `ik` | twist-locked two-bone leg & arm IK (`solve_arm_ik`, `align_palm`, `aim_with_forward`) |
| `neuro` | `PuppetNeuroState`, `WoundMemory`, `GazeTarget` — first-order muscle activation dynamics, fatigue, hit-combo trauma escalation, wound memory, gaze arbitration, impact debouncing |
| `pose` | captures the animated target pose and writes the blended pose back into the bones |
| `sense` | full Avian contact manifolds (normals, points, impulses, approach speeds, impulse-weighted CoP), `SpatialQuery` probes (ground clearance, slopes ahead, approaching walls), vestibular accel, IPM capture point & flywheel region, centroidal angular momentum |
| `drive` | `MuscleJoint`: XPBD compliant constraint with Hill-type force–velocity modulation, anisotropic swing/twist compliance & implicit active + passive viscoelastic velocity damping |
| `muscle` | turns resolved drive + neuromuscular activation into muscle settings, generalized arm & leg Jacobian-transpose virtual forces (`VirtualForce`), gravity compensation, slope-conforming planted feet and pins |
| `behaviour` | `BodyBalance`, `StaggerFall`, `HitReaction` (`HitKind` + 6 anatomical zones + in-place stacking), `BumpReaction`, `WallBrace`, `ReachForWound`, `ArmsWindmill`, multi-phase `CatchFall`, `PhysicalGetUp`, `BodyWrithe`, `Relax` (progressive `DyingCollapse`) |
| `state` | `Animated → Active → Falling → Down → GettingUp`, `Dead`; commands (`PuppetCommandsExt`) and `PuppetEvent` messages (`BodyImpact`, `StepTaken`, `WallBraced`, `Tripped`, `GettingUp`, `RootMoved`, `ModeChanged`) |
| `policy` | hooks for learned control: `PuppetObservation`, `PuppetAction`, `PuppetPolicy`, `PolicyBehaviour`; `ml` feature: `PuppetRecorder` (`.jsonl`) |
| `debug` | gizmos (`gizmos` feature: bodies, CoM, support hull, CoP, ground normal, wall-brace plane, virtual forces, contact spikes, gaze ray) |
| `skeleton` | a procedural UE-named test skeleton |

### Things that were learnt the hard way

* **Muscles live inside the solver.** A joint servo applied as an external
  torque is explicit and ignores its neighbours: a light pelvis between
  three loaded joints rings and explodes at any gain. `MuscleJoint` is an
  XPBD compliant constraint (stiffness = 1/compliance) solved per substep
  with the joints, so any gain is stable.
* **Damp at velocity level.** XPBD only updates velocities after the whole
  substep, so position-level damping lets every muscle on a body damp the
  same stale velocity (three of them overshoot). `damp_muscles` runs in
  Avian's damping stage, Gauss–Seidel on current velocities.
* **Feet need help.** Contacts are solved before the joints and with the
  foot's own mass: a 1 kg foot under 70 kg sinks and rolls on its soft
  contact. Feet are 4× heavier (taken from the thighs, `foot_mass_scale`),
  and a *planted* foot is held level from a static anchor with at most the
  torque the sole could transmit (normal load × half the foot).
* **Balance the trunk through the stance hip** (SIMBICON): the hip muscle's
  target places the pelvis upright over the planted leg; an external torque
  fighting the hip muscle does nothing.
* **Implicit virtual model control.** Jacobian-transpose virtual forces
  (`τ = Jᵀ F + τ_L`, Coros et al. 2010 / Geijtenbeek et al. 2012) are folded
  into `MuscleJoint.target` as an equivalent angular offset (`Δθ = -τ / k`)
  so the XPBD solver applies them implicitly per substep — avoiding 60 Hz
  explicit-torque ringing while respecting friction cones, knee hinge axes,
  and foot-tipping quality.
* **Twist-locked two-bone IK.** Locking femoral/tibial twist to the heading
  bending plane (`aim_with_forward`) and commanding the knee in the thigh's
  local frame keeps the revolute knee hinge aligned during straddled stance,
  preventing lateral foot drift after landing.

### Honest physics vs. assistance

`upright_assist` (default 0.15 of `M·g·H`) is the "helper" of game
balancers: an external torque keeping the trunk upright while balancing.
`balance_assist` pins the pelvis to the animated pose (fights stepping).
Set both to 0 for honest physics.

## Status

* ✅ Rig from UE/Mixamo skeletons, asymmetric anatomical `SphericalJoint` cones + `RevoluteJoint` limits, mass distribution, Hill force–velocity muscles, anisotropic swing/twist compliance, neuromuscular activation dynamics & passive soft-tissue viscoelasticity.
* ✅ Muscles track the animation; animated mode pins bodies to it; physics ⇄
  animation blend per part, written back to the bones.
* ✅ Terrain-adaptive balance: recovers multidirectional pushes (`±Z` up to ~1.0 m/s / 70–75 N·s, `±X` lateral up to 40 N·s) and ramps via IPM stepping, stance-knee reach yield, implicit leg VMC, mid-swing obstacle trip detection, and slope-conforming planted feet.
* ✅ Proprioception & spatial awareness: full contact manifolds (`BodyImpact` events, impulse-weighted CoP), ground clearance & slope-ahead raycasts, approaching-wall detection, vestibular accelerations, IPM capture point, and centroidal angular momentum.
* ✅ Multi-phase falling & bracing: `WallBrace` catches shoves against nearby walls with twist-locked arm IK + arm `VirtualForce`; `CatchFall` handles `AirborneFlail` (high free-fall), `ProtectiveTuck` (high-speed tumble), and directional `Forward`/`Sideways`/`Backward` break-falls (chin tuck, lateral head righting, palm alignment, compliant elbow braking).
* ✅ Physical get-up & dying collapse: 3-stage `PhysicalGetUp` rises from both supine and prone using hand-plant + arm push-up `VirtualForce` + lunge squat into pin handoff; `Kill` runs progressive anatomical `DyingCollapse` (legs buckle first, spine second, arms/head trail) before settling with passive damping.
* ✅ Anatomical combat & crowd reactions: `HitReaction` (`HitKind::{Blunt, Slash, Pierce, Blast}`, sensorimotor latency, 6 distinct anatomical zones — head whip + face-guard, chest whiplash, abdomen forward jackknife, pelvis sweep, leg knee-buckle, arm spin + thoracic twist — plus in-place stacking & combo trauma escalation), `BumpReaction` (shoulder-slip yaw + orienting head glance), `ReachForWound` injury clutching, and wound-biased `BodyWrithe`.
* ✅ ML hooks (observation/action, recorder) — no trained policy yet.

Tests: `cargo test -p bevy_ext_puppet --release` (21 unit + 38 deterministic 60 Hz headless physics tests). `--test puppet_sim trace -- --ignored --nocapture` prints a frame trace (`PUPPET_TRACE_PUSH`, `PUPPET_TRACE_PUSH_X`, `PUPPET_TRACE_LEGS`, `PUPPET_TRACE_JOINTS`…).

## Integrating in a game

* Add `PhysicsPlugins` **before** `PuppetPlugin`.
* For skinned meshes with CPU-readable positions, joint weights and inverse
  bind poses, the rig fits per-part convex colliders when built. Falls, corpses
  and get-ups use these surfaces for contact. Parts without usable skin data
  keep their anatomical colliders.
* The puppet's bodies are on collision layer bit 30; keep the character
  controller's capsule out of it (or the bodies fight it).
* `PuppetEventKind::RootMoved` — when recovering or getting up, the puppet entity
  is moved under the body; a character controller should follow.
* Play a get-up clip on `PuppetEventKind::GettingUp { face_up }`.
