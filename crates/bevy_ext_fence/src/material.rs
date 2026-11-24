//! Um `StandardMaterial` por conjunto de texturas do pool.
//!
//! `assets/textures/<conjunto>/{albedo,normal,orm}.ktx2` — a convenção glTF
//! que o `StandardMaterial` do Bevy 0.20 espera. O `orm` é o mapa combinado
//! (R = oclusão, G = roughness, B = metálico): a mesma imagem alimenta a
//! oclusão e a roughness, e o canal azul a zero é o que impede a madeira de
//! parecer metal — o `metallic_roughness_texture` lê o azul, pelo que lá não
//! pode ir um mapa de roughness em tons de cinzento. A roughness já vem
//! remapeada para a faixa mate do ART.md (0,78–0,95), por isso o factor do
//! material fica a um. Tudo repete (REPEAT + mipmaps + anisotropia, pelo
//! `bevy_ext_gltf::textures`, o único dono do sampler). As ferragens são ferro
//! batido liso: cor, sem textura.

use std::collections::HashMap;

use bevy::image::ImageLoaderSettings;
use bevy::prelude::*;
use bevy_ext_gltf::textures::{WorldTiledTextures, load_tiled_image};

/// Os materiais carregados, partilhados por todas as cercas.
#[derive(Resource, Default)]
pub struct FenceMaterials {
    sets: HashMap<String, Handle<StandardMaterial>>,
    iron: Option<Handle<StandardMaterial>>,
}

impl FenceMaterials {
    /// O material de um conjunto de madeira (ou corda). Carrega os mapas uma
    /// vez; `tiled` é o registo do sampler repetido — dá para carregar sem ele
    /// (a textura carrega na mesma, só não repete).
    pub fn wood(
        &mut self,
        set: &str,
        root: &str,
        server: &AssetServer,
        tiled: Option<Mut<WorldTiledTextures>>,
        materials: &mut Assets<StandardMaterial>,
    ) -> Handle<StandardMaterial> {
        self.wood_from(set, root, "ktx2", server, tiled, materials)
    }

    /// Como [`Self::wood`], com a extensão dos ficheiros (`ktx2` no jogo;
    /// `webp` onde o carregador não lê KTX2 — é o caso do editor).
    #[allow(clippy::too_many_arguments)]
    pub fn wood_from(
        &mut self,
        set: &str,
        root: &str,
        ext: &str,
        server: &AssetServer,
        mut tiled: Option<Mut<WorldTiledTextures>>,
        materials: &mut Assets<StandardMaterial>,
    ) -> Handle<StandardMaterial> {
        let root = root.trim().trim_end_matches('/').trim_start_matches('/');
        let key = format!("{root}/{set}.{ext}");
        if let Some(handle) = self.sets.get(&key) {
            return handle.clone();
        }
        let url = |map: &str| format!("{}/{}/{}.{ext}", root, set, map);

        // O albedo é sRGB; os mapas de dados são lineares, apesar de serem imagens.
        let albedo = match tiled.as_mut() {
            Some(tiled) => load_tiled_image(server, tiled.as_mut(), &url("albedo")),
            None => server.load::<Image>(url("albedo")),
        };
        let mut data = Vec::with_capacity(2);
        for map in ["normal", "orm"] {
            let handle = server
                .load_builder()
                .with_settings(|settings: &mut ImageLoaderSettings| settings.is_srgb = false)
                .load(url(map));
            if let Some(tiled) = tiled.as_mut() {
                tiled.register(handle.id());
            }
            data.push(handle);
        }
        let [normal, orm] = <[Handle<Image>; 2]>::try_from(data).expect("dois mapas de dados");

        let handle = materials.add(StandardMaterial {
            base_color_texture: Some(albedo),
            normal_map_texture: Some(normal),
            // glTF ORM: R = oclusão, G = roughness, B = metálico (zero na madeira).
            occlusion_texture: Some(orm.clone()),
            metallic_roughness_texture: Some(orm),
            metallic: 1.0,
            perceptual_roughness: 1.0,
            ..Default::default()
        });
        self.sets.insert(key, handle.clone());
        handle
    }

    /// Ferro batido: liso, mate e escuro.
    pub fn iron(&mut self, color: Color, materials: &mut Assets<StandardMaterial>) -> Handle<StandardMaterial> {
        self.iron.get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: color,
                metallic: 0.55,
                perceptual_roughness: 0.62,
                ..Default::default()
            })
        }).clone()
    }

    /// Esquece tudo (troca de cena): os handles vivem nos `Assets`.
    pub fn clear(&mut self) {
        self.sets.clear();
        self.iron = None;
    }
}
