# bevy_ext_gltf

GLB reading for Bevy 0.20 with the compression/quantization extensions the
stock loader does not support:

- `EXT_meshopt_compression` — `ATTRIBUTES`, `TRIANGLES` and `INDICES` modes,
  with `NONE`/`OCTAHEDRAL`/`QUATERNION`/`EXPONENTIAL` filters.
- `KHR_mesh_quantization` — widens `POSITION`/`NORMAL`/`TANGENT` from
  i8/u8/i16/u16 (normalized or not) into packed `f32` buffer views.
- `KHR_texture_basisu` — lifts the texture source out of the extension so the
  Bevy `gltf` loader finds it; KTX2/UASTC transcoding stays with Bevy's
  `basis-universal` feature.
- UASTC `RRR` — re-labels the channel layout to `RGB` to avoid Bevy's BC4 path
  (see [Workarounds](#workarounds)).

```toml
[dependencies]
bevy_ext_gltf = "0.1"
```

```rust
use bevy::prelude::*;

let mut app = App::new();
bevy_ext_gltf::register_asset_source(&mut app, vec!["assets".into()]);
app.add_plugins(DefaultPlugins);
```

The reader must be registered **before** `AssetPlugin` is added. The crate is
runtime-agnostic: it wraps the default asset source and does not depend on a
game or an editor.

## How it reads

The rewrite is a single pass. Every size it needs comes from the JSON, so the
final offsets are assigned before any byte moves: the output GLB is allocated
once and each compressed view is decoded straight into it. Files that need
nothing are returned borrowed (`decode_glb` returns a `Cow`), so the reader
hands its own buffer to the loader instead of duplicating it. Everything that
is not a glTF streams through untouched.

`examples/decode_bench` measures it on a real pool:

```console
$ cargo run -p bevy_ext_gltf --release --example decode_bench -- \
      --root /path/to/assets/public [--limit N] [--json]
files 676  source 2324.4 MB -> decoded 2561.0 MB (+10.2%)
total 676.7 ms  p50 0.18 ms  p95 4.54 ms  3434.9 MB/s
peak RSS 68.7 MB
```

With `--features legacy` it also runs the pre-rewrite decoder on the same bytes,
asserts the outputs are identical and reports the side-by-side timing.

## Decode policy

The workarounds below are policy, not literals:

| Variable | Default | Effect |
|---|---|---|
| `BEVY_EXT_GLTF_VALIDATE=1` | off | Run the `gltf` crate's validator on load |
| `BEVY_EXT_GLTF_UASTC_RELABEL=0` | on | Keep single-channel UASTC as `RRR` |
| `BEVY_EXT_GLTF_DEQUANTIZE=0` | on | A/B only — meshes will panic |
| `BEVY_EXT_GLTF_MIPS` | `background` | `inline`, `background` or `off` |
| `BEVY_EXT_GLTF_STATS=1` | off | Log the decode counters every 5 s |
| `BEVY_EXT_GLTF_POOL_CACHE=0` | on | Disable the existence memo |

`bevy_ext_gltf::decode_stats()` returns the same counters the log prints, and
the `GltfStatsPlugin` adds the log system.

## Optional helpers

- `bounds` (default): `GlbBounds::read`/`read_bytes` — scene AABB from the GLB
  JSON, without a renderer.
- `textures` (default): CPU mipmaps and the world-tiled sampler for PNG/RGBA
  images. Large chains are built on the compute pool; `patch_image` stays the
  synchronous entry point and the single writer of the sampler.

## Workarounds

Two of them cannot be removed without an upstream change, so they are
documented and switchable instead of hidden:

- **UASTC `RRR` → `RGB`.** `bevy_image`'s KTX2 path sizes the slice handed to
  the UASTC transcoder with the *target* format's block size
  (`bevy_image-0.20.1/src/ktx2.rs:181-193`). A UASTC block is always 16 bytes
  and a BC4 block is 8, so `UASTC_RRR` (a one-channel AO/roughness map) fails
  with `TranscodeFailed`. Re-labelling it `RGB` takes the BC7 path, where the
  block sizes agree; the cost is twice the VRAM for those maps. Counted in
  `DecodeStats::uastc_relabels`.
- **Quantized attributes → `f32`.** `Mesh::insert_attribute` panics unless the
  values match the attribute's declared format
  (`bevy_mesh-0.20.1/src/mesh.rs:471`), and `ATTRIBUTE_POSITION` is
  `Float32x3`, so the widening is mandatory regardless of what the loader
  supports.

## Limits

Only self-contained GLB files are decoded; `.gltf` with external buffers,
sparse accessors, Draco and multi-buffer GLBs are out of scope, and the reader
warns (once, by name) when it meets one. ETC1S/BasisLZ KTX2 is not transcoded
by Bevy; the audit tools warn.

The tests use two small real GLBs in `tests/fixtures/` (meshopt +
quantization + KTX2/basisu) when `VAEL_ROOT` is unset, so the decoder path runs
in CI without the game checkout. Set `VAEL_ROOT` to the game checkout to run
the pool tests, and add `--features legacy` for the byte-parity gate.

## Bevy version

Built against Bevy 0.20.1. The crate enables `bevy_asset`, `bevy_gltf`,
`bevy_image`, `bevy_log` and `bevy_render`; the host app owns the
renderer/window features.

## License

MIT OR Apache-2.0.
