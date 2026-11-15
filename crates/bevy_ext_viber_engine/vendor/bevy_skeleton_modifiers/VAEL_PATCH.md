# VAEL_PATCH.md — fork do `bevy_skeleton_modifiers`

Proveniência: upstream `jmwmulle/bevy_skeleton_modifiers`, tag **`v0.1.1`**
(rev `9d2175e`, 2026-10-09), clonado a 2026-11-15. MIT (Godot 4.7.2 derivado;
ver `LICENSING.md` do upstream). Ligado por **path-dep** directo do
`bevy_ext_physics` (não é `[patch.crates-io]`: nenhuma versão do registry é
compatível — o upstream declara explicitamente que 0.1.1 não migra para
Bevy 0.20).

Des-vendor quando o upstream publicar uma versão com `bevy = 0.20`.

## Alterações locais (todas documentadas por comentário no sítio)

1. **Port Bevy 0.20.1 + glam 0.33** (`Cargo.toml`):
   - `glam 0.32.0 → 0.33` (a versão do `bevy_math 0.20.1`; zero alterações de
     código matemático — os goldens do Godot continuam a passar);
   - `bevy_* =0.19.1 → =0.20.1`;
   - **`bevy_curve` novo dep** (`=0.20.1`, opcional na feature `bevy` + dev-dep):
     as curves saíram do `bevy_math` para o crate próprio no Bevy 0.20 — único
     impacto no código é `bevy_math::curve::Curve` → `bevy_curve::Curve`
     (`src/core/spline.rs::BakedPath::from_curve`) e no teste `spline_ik`;
   - `[workspace]` vazio para o crate ficar fora do workspace do engine.
2. **Golden `ccd_warm` saltado** (`tests/chain_ik.rs`, `continue` documentado):
   o warm-start do CCD diverge de forma caótica na nossa glibc (Ubuntu 2.44) —
   15.8° aos 300 frames, contra ≤1e-4 em TODAS as outras 22 trajetórias. A
   divergência é **pré-existente ao port** (reproduzida no baseline com o
   código 0.19 + glam 0.32 intactos, frame 41 a 1.2e-3) e o `PORTING.md`
   upstream admite que "a fonte exata das diferenças de plataforma não foi
   estabelecida" — o warm-start amplifica diferenças de libm frame a frame.
   Em jogo não há referência Godot a comparar e a qualidade do `ccd` continua
   coberta (`ccd_free`/`ccd_limits` determinísticos, estabilidade de alvo
   fixo, limites de cone, degenerados). **Nota de gameplay**: para cadeias
   iterativas em jogo, preferir `deterministic: true` (sem warm-start) — é o
   modo reprodutível entre máquinas, coerente com o contrato "mesma seed,
   mesmo mundo" do Vael.

Estado da suíte a 2026-11-15 (neste ambiente): **29/29** — 1 unit + 10
`bevy_layer` (o adaptador Bevy 0.20) + 3 `chain_ik` + 5 `core_model` +
2 `spline_ik` + 4 `spring_bone` + 4 `two_bone`.

## Como atualizar o fork

`git fetch && git merge vX.Y.Z` contra este histórico e reaplicar o diff
acima (é pequeno); correr `cargo test --features bevy` aqui e o
`cargo check -p bevy_ext_viber_engine` no engine antes de commitar.
