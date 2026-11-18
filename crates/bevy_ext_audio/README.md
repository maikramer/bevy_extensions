# bevy_ext_audio

Runtime audio for Bevy games — the Viber audio stack extracted as a package:
SFX one-shots with distance attenuation, ambient loops (water, waterfalls,
rain), typed kira buses, mixer sync, per-bus ducking, reverb, zone/crossfade
BGM layers and combat music.

```toml
[dependencies]
bevy_ext_audio = "0.1"
```

```rust
use bevy_ext_audio::{AudioPlugin, AudioPaths, AudioWorldState};

app.add_plugins(AudioPlugin);
app.insert_resource(AudioPaths::default()); // sfx_dir; the plugin seeds the default
```

## What it contains

- `sfx` — `SfxHandles` preload of every `SfxClip`, playback of
  `SfxEvent`/`SfxLoudEvent` with pitch jitter and 40 m attenuation.
- `ambience` — water/waterfall/rain loops with quadratic distance fades.
- `music` — typed buses (`MusicBus`/`SfxBus`/`VoiceBus`/`AmbienceBus`),
  `mixer_sync` (sliders reach mid-flight sounds), `AudioDuck` (stacking,
  self-expiring), `AudioReverb` presets, `ScriptSoundLoops`, BGM layers with
  `bgm_zone`/`auto_zone` crossfade and `CombatMusicState`.
- `names` — the clip-name registry (`"hit"` → `SfxClip::Hit`), shared with
  the script layer.
- `host` — the resources the host app publishes: `AudioPaths` (sfx_dir),
  `AudioWorldState` (rain / scene id / biome id) and `HostWaterBodies`
  (the `WaterBodies` trait for terrain water).

## Plugins

Add `AudioPlugin` (everything) **or** `SfxPlugin`/`MusicPlugin` separately —
never both, Bevy does not deduplicate systems.

## Host wiring (kira backend)

The plugin owns systems and state only; the audio backend stays host-side:

```rust
app.add_plugins(bevy_kira_audio::AudioPlugin);
bevy_ext_audio::music::install_mixer(&mut app);
```

Without channels the systems are silent no-ops, which keeps headless test
apps green.

## Bevy version

Built against Bevy 0.20 with `bevy_kira_audio` 0.27 (`ogg`). The shared types
(`SfxClip`, `SfxEvent`, `AudioMixerSettings`, …) live in `bevy_ext_core` and
keep the historical `viber::…` reflection paths.
