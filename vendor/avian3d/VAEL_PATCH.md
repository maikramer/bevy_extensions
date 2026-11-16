# avian3d 0.8.0 — patch do Vael

Cópia do `avian3d` 0.8.0 publicado (crates.io) com UMA mudança, ligada por
`[patch.crates-io]` no `Cargo.toml` do Vael.

## `propagate_collider_transforms` salta subárvores estáticas

`src/collision/collider/collider_transform/plugin.rs`. O upstream percorre,
em cada passo de física, TODA a hierarquia de cada raiz que tenha um collider
descendente (`AncestorMarker<ColliderMarker>`) — no Vael a raiz do mundo
estático tem ~86k descendentes, e o sistema custava ~3.6 ms/frame a 26 fps
(perf, 2026-11-15).

O patch lê o `TransformTreeChanged` do Bevy (marcado pelo `mark_dirty_trees`
que o próprio avian corre no mesmo set `PhysicsTransformSystems::Propagate`;
o sistema passou a correr `.after(mark_dirty_trees)`) e não desce para um
filho quando nem o pai mudou nem a árvore do filho foi marcada desde a última
corrida. Fallback para a travessia completa quando:

- `StaticTransformOptimizations` está desligado (o `mark_dirty_trees` não marca);
- algum `ColliderTransform` foi inserido neste passo (collider construído
  depois do `Transform`, ex.: `ColliderConstructor` assíncrono — não marca a
  árvore).

Des-vendor quando o upstream fizer o mesmo (ou quando subir de versão: reaplicar).
