//! Escrita genérica por reflexão (`patch_component`): o maquinaria que
//! `set_scale`/`set_visible`/`set_pose` usam para aplicar comandos de script
//! a componentes registados (`TypePath`), portada do `viber::luau::ecs` do
//! monólito (2026-11-15, onda final do DELETE-MONOLITH — a TABELA `viber.ecs`
//! e o espelho por frame já tinham morrido; sobrou o caminho de escrita).
//!
//! - [`patch_component`]: patch parcial por reflexão (struct/tuplo/enum);
//! - [`insert_component`]/[`remove_component`]: componente inteiro;
//! - [`EcsValue`]: a forma PLANA do valor (independente de VM) que os
//!   comandos enfileirados transportam; [`value_from_reflect`] converte.
//!
//! Os testes do maquinaria (contra os tipos do anfitrião) ficaram no
//! monólito, com o consumidor (`bevy_ext_viber_engine::luau::ecs`).


use bevy::ecs::reflect::ReflectComponent;
use bevy::prelude::*;
use bevy::reflect::{
    PartialReflect, ReflectFromReflect, ReflectMut, ReflectRef, TypeInfo, TypeRegistration,
    TypeRegistry,
    enums::{DynamicEnum, DynamicVariant, Enum, EnumInfo, VariantType},
    std_traits::ReflectDefault,
    structs::DynamicStruct,
    tuple::DynamicTuple,
    tuple_struct::DynamicTupleStruct,
};


/// Tipos que o Luau pode ver/escrever por `viber.ecs` (`viber.ecs.types()`).
///
/// Curada: só dados de JOGO que um script tem motivo para inspecionar. Ficam
/// de fora caches (GPU, Rapier, VM), tipos enormes (`CharacterAnimator`) e os
/// componentes de transformação/visibilidade — esses têm verbos próprios
/// (`viber.position`, `viber.set_pose`, `viber.set_visible`) e o espelho de
/// todos os `Transform` de um mundo grande (≈79k entidades no Vael) seria
/// desperdício por frame.
pub const WHITELIST: &[&str] = &[
    // Vitais e combate
    "viber::vitals::Health",
    "viber::vitals::Hostile",
    "bevy_ext_viber_engine::projectile::Projectile",
    "bevy_ext_viber_engine::spawn_gate::SpawnGated",
    // Jogador e IA
    "viber::player::Player",
    "viber::player::TeleportSettle",
    "viber::ai::AiLocomotion",
    "viber::ai::EnemyCreature",
    "viber::ai::WanderState",
    // Ligação entidade↔script
    "viber::luau::components::LuaScriptRef",
    "viber::luau::components::ScriptProps",
    "viber::luau::components::Groups",
    "viber::luau::components::SignalConnections",
    "viber::luau::components::ScriptActivation",
    "viber::luau::components::ScriptInteraction",
    // Câmara
    "viber::camera::BaseFov",
];

/// Caminho do `Transform` (escritas internas de `set_pose`/`set_scale`; NÃO
/// entra no espelho Lua).
pub const TRANSFORM_PATH: &str = "bevy_transform::components::transform::Transform";

/// Caminho da `Visibility` (escritas internas de `set_visible`).
pub const VISIBILITY_PATH: &str = "bevy_camera::visibility::Visibility";

/// Entidades vazias reservadas por frame para `viber.ecs.spawn` (mínimo).
pub const MIN_SPAWN_POOL: usize = 8;

/// Valor refletido convertido para uma forma independente da VM.
///
/// `Map` = struct com campos nomeados; `Seq` = tuplo/lista/array. Os nomes
/// seguem a API existente (`x`/`y`/`z`, `r`/`g`/`b`/`a`) para uma tabela Lua
/// poder voltar a entrar em `set` sem tradução.
#[derive(Debug, Clone, PartialEq)]
pub enum EcsValue {
    /// Struct/componente unitário (`Hostile`) ou variante sem dados.
    Unit,
    /// Ausência (`Option::None`, campo não representável). Em Lua vira `nil`.
    Nil,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Str(String),
    Vec2([f32; 2]),
    Vec3([f32; 3]),
    Quat([f32; 4]),
    Color([f32; 4]),
    /// Bits da entidade (i64, como `viber.self_id()`).
    Entity(i64),
    /// Variante de enum sem payload (`Faction::Enemy` → `"Enemy"`).
    Enum(String),
    Map(Vec<(String, EcsValue)>),
    Seq(Vec<EcsValue>),
}


// ------------------------------------------------------------------ registo

/// `AppTypeRegistry` do mundo (clone do `Arc`; sem emprestar o `World`).
pub fn registry_arc(world: &World) -> Result<bevy::reflect::TypeRegistryArc, String> {
    world
        .get_resource::<bevy::ecs::reflect::AppTypeRegistry>()
        .map(|registry| registry.0.clone())
        .ok_or_else(|| "AppTypeRegistry ausente — o ReflectPlugin não está instalado".to_string())
}

fn registration<'a>(
    registry: &'a TypeRegistry,
    type_path: &str,
) -> Result<&'a TypeRegistration, String> {
    registry.get_with_type_path(type_path).ok_or_else(|| {
        format!("tipo '{type_path}' não está no AppTypeRegistry (vê viber.ecs.types())")
    })
}

fn component_reflection<'a>(
    registry: &'a TypeRegistry,
    entity: Entity,
    world: &World,
    type_path: &str,
) -> Result<&'a ReflectComponent, String> {
    // O MESMO type_path pode ter VÁRIOS registos no grafo do teste/app: o
    // contrato unificado (`bevy_ext_core`) e variantes do mesmo tipo de
    // compilações distintas (deps normal vs dev — o `cargo tree --duplicate`
    // mostra os gémeos). O mapa `type_path_to_id` guarda só o ÚLTIMO, e o
    // `reflect_mut` falhava em silêncio (`Ok(false)` num patch) quando a
    // entidade carregava o outro. Resolve pelo TypeId do componente REAL na
    // entidade; sem ela (ou sem match), cai no último de sempre.
    let candidates: Vec<&TypeRegistration> = registry
        .iter()
        .filter(|r| r.type_info().type_path() == type_path)
        .collect();
    if candidates.len() > 1 {
        if let Ok(entity_ref) = world.get_entity(entity) {
            for component_id in entity_ref.archetype().components() {
                let Some(info) = world.components().get_info(*component_id) else {
                    continue;
                };
                let Some(type_id) = info.type_id() else {
                    continue;
                };
                if let Some(r) = candidates
                    .iter()
                    .find(|r| r.type_info().type_id() == type_id)
                {
                    return r.data::<ReflectComponent>().ok_or_else(|| {
                        format!(
                            "'{type_path}' está registado mas não é um componente refletido"
                        )
                    });
                }
            }
        }
    }
    let registration = registration(registry, type_path)?;
    registration
        .data::<ReflectComponent>()
        .ok_or_else(|| format!("'{type_path}' está registado mas não é um componente refletido"))
}

// ------------------------------------------------------------- leitura

/// Converte um valor refletido para [`EcsValue`] (structs um nível abaixo).
pub fn value_from_reflect(value: &dyn PartialReflect) -> EcsValue {
    value_from_reflect_at(value, 0)
}

const MAX_STRUCT_DEPTH: usize = 2;

fn value_from_reflect_at(value: &dyn PartialReflect, depth: usize) -> EcsValue {
    // Tipos com representação canónica na API Lua.
    if let Some(entity) = value.try_downcast_ref::<Entity>() {
        return EcsValue::Entity(entity.to_bits() as i64);
    }
    if let Some(vec) = value.try_downcast_ref::<Vec2>() {
        return EcsValue::Vec2([vec.x, vec.y]);
    }
    if let Some(vec) = value.try_downcast_ref::<Vec3>() {
        return EcsValue::Vec3([vec.x, vec.y, vec.z]);
    }
    if let Some(quat) = value.try_downcast_ref::<Quat>() {
        let [x, y, z, w] = quat.to_array();
        return EcsValue::Quat([x, y, z, w]);
    }
    if let Some(color) = value.try_downcast_ref::<Color>() {
        let color = color.to_srgba();
        return EcsValue::Color([color.red, color.green, color.blue, color.alpha]);
    }
    // Escalares.
    if let Some(value) = value.try_downcast_ref::<bool>() {
        return EcsValue::Bool(*value);
    }
    macro_rules! scalar {
        ($($ty:ty => $variant:ident),* $(,)?) => {$(
            if let Some(value) = value.try_downcast_ref::<$ty>() {
                return EcsValue::$variant((*value) as _);
            }
        )*};
    }
    scalar!(
        i8 => Int, i16 => Int, i32 => Int, i64 => Int, isize => Int,
        u8 => UInt, u16 => UInt, u32 => UInt, u64 => UInt, usize => UInt,
        f32 => Float, f64 => Float,
    );
    if let Some(value) = value.try_downcast_ref::<String>() {
        return EcsValue::Str(value.clone());
    }

    match value.reflect_ref() {
        ReflectRef::Struct(struct_value) => {
            if depth >= MAX_STRUCT_DEPTH {
                return EcsValue::Str(format!("<{}>", value.reflect_type_path()));
            }
            let mut fields = Vec::with_capacity(struct_value.field_len());
            for (name, field) in struct_value.iter_fields() {
                fields.push((name.to_string(), value_from_reflect_at(field, depth + 1)));
            }
            EcsValue::Map(fields)
        }
        ReflectRef::TupleStruct(tuple) => {
            if tuple.field_len() == 0 {
                return EcsValue::Unit;
            }
            EcsValue::Seq(
                (0..tuple.field_len())
                    .filter_map(|index| tuple.field(index))
                    .map(|field| value_from_reflect_at(field, depth + 1))
                    .collect(),
            )
        }
        ReflectRef::Tuple(tuple) => EcsValue::Seq(
            (0..tuple.field_len())
                .filter_map(|index| tuple.field(index))
                .map(|field| value_from_reflect_at(field, depth + 1))
                .collect(),
        ),
        ReflectRef::List(list) => EcsValue::Seq(
            (0..list.len())
                .filter_map(|index| list.get(index))
                .map(|field| value_from_reflect_at(field, depth + 1))
                .collect(),
        ),
        ReflectRef::Array(array) => EcsValue::Seq(
            (0..array.len())
                .filter_map(|index| array.get(index))
                .map(|field| value_from_reflect_at(field, depth + 1))
                .collect(),
        ),
        ReflectRef::Enum(enum_value) => {
            let name = enum_value.variant_name();
            if name == "None" && enum_value.field_len() == 0 {
                return EcsValue::Nil;
            }
            if name == "Some"
                && let Some(field) = enum_value.field_at(0)
            {
                return value_from_reflect_at(field, depth + 1);
            }
            if enum_value.variant_type() == VariantType::Unit {
                return EcsValue::Enum(name.to_string());
            }
            let mut fields = vec![("variant".to_string(), EcsValue::Enum(name.to_string()))];
            for (index, field) in enum_value.iter_fields().enumerate() {
                fields.push((
                    index.to_string(),
                    value_from_reflect_at(field.value(), depth + 1),
                ));
            }
            EcsValue::Map(fields)
        }
        // Mapas/sets/opacos não têm forma canónica nesta API (um script lê o
        // que é dado de jogo; caches são explicitamente para ficar de fora).
        _ => EcsValue::Str(format!("<{}>", value.reflect_type_path())),
    }
}

// ------------------------------------------------------------- escrita

/// Resultado de um patch: o componente existia? (`false` = entidade sem ele).
pub type Patched = bool;

/// Aplica um patch parcial a um componente já existente.
///
/// `type_path` não é validado contra a [`WHITELIST`]: os verbos internos
/// (`set_pose`/`set_scale`/`set_visible`) usam-no para `Transform`/`Visibility`.
pub fn patch_component(
    world: &mut World,
    entity: Entity,
    type_path: &str,
    patch: &EcsValue,
) -> Result<Patched, String> {
    let registry = registry_arc(world)?;
    let registry = registry.read();
    let reflection = component_reflection(&registry, entity, world, type_path)?;
    let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
        return Err(format!("entidade {entity:?} não existe"));
    };
    let Some(mut value) = reflection.reflect_mut(&mut entity_mut) else {
        return Ok(false);
    };
    match value.reflect_mut() {
        ReflectMut::Struct(struct_value) => {
            let fields = match patch {
                EcsValue::Unit | EcsValue::Nil => return Ok(true),
                EcsValue::Map(fields) => fields,
                _ => {
                    return Err(format!(
                        "{type_path}: o patch tem de ser uma tabela {{campo = valor}}"
                    ));
                }
            };
            for (name, field_value) in fields {
                let field = struct_value
                    .field_mut(name)
                    .ok_or_else(|| format!("{type_path}: campo '{name}' não existe"))?;
                set_from_value(
                    field,
                    field_value,
                    &format!("{type_path}.{name}"),
                    &registry,
                )?;
            }
        }
        // Componente tuplo (`BaseFov`, `Groups`): vazio é no-op; um array
        // posicional escreve os campos por índice.
        ReflectMut::TupleStruct(tuple) => match patch {
            EcsValue::Unit | EcsValue::Nil => {}
            EcsValue::Map(fields) if fields.is_empty() => {}
            EcsValue::Seq(values) => {
                for (index, field_value) in values.iter().enumerate() {
                    let field = tuple
                        .field_mut(index)
                        .ok_or_else(|| format!("{type_path}: campo {index} não existe"))?;
                    set_from_value(
                        field,
                        field_value,
                        &format!("{type_path}.{index}"),
                        &registry,
                    )?;
                }
            }
            _ => {
                return Err(format!(
                    "{type_path}: componente tuplo espera um array posicional"
                ));
            }
        },
        // Componente que é ele próprio um enum (`Visibility`): o patch é a
        // variante (`"Hidden"`) ou o payload dela.
        ReflectMut::Enum(_) => {
            set_from_value(&mut *value, patch, type_path, &registry)?;
        }
        _ => {
            return Err(format!(
                "{type_path}: tipo {} não suportado por viber.ecs",
                value.reflect_type_path()
            ));
        }
    }
    Ok(true)
}

/// Insere (ou substitui) um componente construído por reflexão.
pub fn insert_component(
    world: &mut World,
    entity: Entity,
    type_path: &str,
    patch: &EcsValue,
) -> Result<(), String> {
    let registry = registry_arc(world)?;
    let registry = registry.read();
    let reflection = component_reflection(&registry, entity, world, type_path)?;
    let value = build_component(&registry, type_path, patch)?;
    let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
        return Err(format!("entidade {entity:?} não existe"));
    };
    reflection.insert(&mut entity_mut, value.as_partial_reflect(), &registry);
    Ok(())
}

/// Remove um componente; `false` se a entidade não o tinha.
pub fn remove_component(
    world: &mut World,
    entity: Entity,
    type_path: &str,
) -> Result<bool, String> {
    let registry = registry_arc(world)?;
    let registry = registry.read();
    let reflection = component_reflection(&registry, entity, world, type_path)?;
    let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
        return Err(format!("entidade {entity:?} não existe"));
    };
    if !reflection.contains(&entity_mut) {
        return Ok(false);
    }
    reflection.remove(&mut entity_mut);
    Ok(true)
}


// -------------------------------------------------- construir um componente

/// Constrói o valor concreto de `type_path` a partir do patch Lua.
///
/// Preferência: `Default` refletido (o tipo decide os campos ausentes) e só
/// depois `FromReflect` com TODOS os campos fornecidos. Um tipo sem `Default`
/// nem todos os campos devolve erro claro — nunca uma construção a zeros.
fn build_component(
    registry: &TypeRegistry,
    type_path: &str,
    patch: &EcsValue,
) -> Result<Box<dyn Reflect>, String> {
    let registration = registration(registry, type_path)?;
    let info = registration.type_info();

    // Caminho 1: Default do próprio tipo (a forma segura para `Health`,
    // `Player`, `AiLocomotion`, …).
    if let Some(default) = registration.data::<ReflectDefault>() {
        let mut value = default.default();
        apply_patch_to_value(value.as_partial_reflect_mut(), patch, type_path, registry)?;
        return Ok(value);
    }

    // Caminho 2: `FromReflect` exige todos os campos ativos.
    let struct_info = info
        .as_struct()
        .map_err(|_| format!("{type_path}: insert de viber.ecs só suporta componentes struct"))?;
    let fields = match patch {
        EcsValue::Unit | EcsValue::Nil => Vec::new(),
        EcsValue::Map(fields) => fields.clone(),
        _ => {
            return Err(format!(
                "{type_path}: o patch de insert tem de ser uma tabela {{campo = valor}}"
            ));
        }
    };
    for (name, value) in &fields {
        let field = struct_info
            .field(name)
            .ok_or_else(|| format!("{type_path}: campo '{name}' não existe"))?;
        build_partial(field.type_info(), value, &format!("{type_path}.{name}")).map(|_| ())?;
    }
    let missing: Vec<&str> = struct_info
        .field_names()
        .iter()
        .copied()
        .filter(|name| {
            let wanted: &str = name;
            !fields.iter().any(|(field, _)| field.as_str() == wanted)
        })
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "{type_path}: tipo sem Default de reflexão e faltam campos {missing:?} \
             (fornece-os todos no insert)"
        ));
    }
    let from_reflect = registration.data::<ReflectFromReflect>().ok_or_else(|| {
        format!("{type_path}: não é construível por reflexão (sem FromReflect/Default)")
    })?;
    let mut dynamic = DynamicStruct::default();
    dynamic.set_represented_type(Some(info));
    for (name, value) in &fields {
        let field = struct_info
            .field(name)
            .ok_or_else(|| format!("{type_path}: campo '{name}' não existe"))?;
        dynamic.insert_boxed(
            name.clone(),
            build_partial(field.type_info(), value, &format!("{type_path}.{name}"))?,
        );
    }
    from_reflect
        .from_reflect(&dynamic)
        .ok_or_else(|| format!("{type_path}: FromReflect recusou o valor construído"))
}

/// Aplica os campos de um `Map` a um valor concreto já existente.
fn apply_patch_to_value(
    value: &mut dyn bevy::reflect::PartialReflect,
    patch: &EcsValue,
    path: &str,
    registry: &TypeRegistry,
) -> Result<(), String> {
    match patch {
        EcsValue::Unit | EcsValue::Nil => Ok(()),
        EcsValue::Map(fields) if fields.is_empty() => Ok(()),
        EcsValue::Map(fields) => {
            let Ok(struct_value) = value.reflect_mut().as_struct() else {
                return Err(format!(
                    "{path}: o componente não é um struct — passa {{}} ou os campos"
                ));
            };
            for (name, field_value) in fields {
                let field = struct_value
                    .field_mut(name)
                    .ok_or_else(|| format!("{path}: campo '{name}' não existe"))?;
                set_from_value(field, field_value, &format!("{path}.{name}"), registry)?;
            }
            Ok(())
        }
        _ => Err(format!(
            "{path}: o patch tem de ser uma tabela {{campo = valor}}"
        )),
    }
}

/// Constrói um valor dinâmico de um campo a partir do `TypeInfo` do campo.
///
/// Só é usado no caminho sem `Default` (todos os campos são fornecidos).
fn build_partial(
    info: Option<&'static TypeInfo>,
    value: &EcsValue,
    path: &str,
) -> Result<Box<dyn bevy::reflect::PartialReflect>, String> {
    let info = info.ok_or_else(|| format!("{path}: tipo sem TypeInfo (opaco)"))?;
    // Os canónicos de vetor/cor viajam como variantes próprias e são
    // construídos pelo `TypeId` (o `TypeInfo` deles pode ser Struct).
    if matches!(
        value,
        EcsValue::Vec2(_) | EcsValue::Vec3(_) | EcsValue::Quat(_) | EcsValue::Color(_)
    ) {
        return build_opaque(info, value, path);
    }
    match info {
        TypeInfo::Opaque(_) => build_opaque(info, value, path),
        TypeInfo::Struct(struct_info) => {
            let fields = match value {
                EcsValue::Unit | EcsValue::Nil => &[][..],
                EcsValue::Map(fields) => fields.as_slice(),
                _ => return Err(format!("{path}: esperava uma tabela de campos")),
            };
            let mut dynamic = DynamicStruct::default();
            dynamic.set_represented_type(Some(info));
            for (name, field_value) in fields {
                let field = struct_info
                    .field(name)
                    .ok_or_else(|| format!("{path}: campo '{name}' não existe"))?;
                dynamic.insert_boxed(
                    name.clone(),
                    build_partial(field.type_info(), field_value, &format!("{path}.{name}"))?,
                );
            }
            Ok(Box::new(dynamic))
        }
        TypeInfo::Enum(enum_info) => {
            // `Option<T>`: payload direto (`{x,y}`, bits de entidade, …) →
            // `Some`; `nil`/`"none"` → `None`.
            if enum_info.variant("Some").is_some() && enum_info.variant("None").is_some() {
                if matches!(value, EcsValue::Nil)
                    || matches!(value, EcsValue::Str(name) if name.eq_ignore_ascii_case("none"))
                {
                    let index = enum_info.index_of("None").unwrap_or(0);
                    let mut dynamic =
                        DynamicEnum::new_with_index(index, "None", DynamicVariant::Unit);
                    dynamic.set_represented_type(Some(info));
                    return Ok(Box::new(dynamic));
                }
                let variant = enum_info
                    .variant("Some")
                    .ok_or_else(|| format!("{path}: variante Some ausente"))?;
                let tuple_info = variant
                    .as_tuple_variant()
                    .map_err(|_| format!("{path}: variante Some não é tuplo"))?;
                let field = tuple_info
                    .field_at(0)
                    .ok_or_else(|| format!("{path}: Some sem campo"))?;
                let mut tuple = DynamicTuple::default();
                tuple.insert_boxed(build_partial(field.type_info(), value, path)?);
                let index = enum_info.index_of("Some").unwrap_or(0);
                let mut dynamic =
                    DynamicEnum::new_with_index(index, "Some", DynamicVariant::Tuple(tuple));
                dynamic.set_represented_type(Some(info));
                return Ok(Box::new(dynamic));
            }
            let (name, payload) = match value {
                EcsValue::Str(name) => (name.as_str(), None),
                EcsValue::Enum(name) => (name.as_str(), None),
                EcsValue::Nil => ("None", None),
                EcsValue::Map(fields) => {
                    let Some((_, EcsValue::Str(name) | EcsValue::Enum(name))) =
                        fields.iter().find(|(key, _)| key == "variant")
                    else {
                        return Err(format!(
                            "{path}: variante de enum precisa de {{variant = \"Nome\"}}"
                        ));
                    };
                    (name.as_str(), Some(value))
                }
                _ => {
                    return Err(format!("{path}: enum espera o nome da variante (string)"));
                }
            };
            let variant = enum_info.variant(name).ok_or_else(|| {
                format!(
                    "{path}: variante '{name}' não existe em {}",
                    info.type_path()
                )
            })?;
            let index = enum_info.index_of(name).ok_or_else(|| {
                format!(
                    "{path}: variante '{name}' não existe em {}",
                    info.type_path()
                )
            })?;
            let dyn_variant = match variant.variant_type() {
                VariantType::Unit => DynamicVariant::Unit,
                VariantType::Tuple => {
                    let tuple_info = variant
                        .as_tuple_variant()
                        .map_err(|_| format!("{path}: variante '{name}' não tem campos"))?;
                    let mut tuple = DynamicTuple::default();
                    let Some(payload) = payload else {
                        return Err(format!(
                            "{path}: variante '{name}' tem payload — usa \
                             {{variant = \"{name}\", \"0\" = …}}"
                        ));
                    };
                    let field = tuple_info.field_at(0).ok_or_else(|| {
                        format!("{path}: variante '{name}' sem campos no TypeInfo")
                    })?;
                    let field_value = field_from_variant_table(payload)?;
                    tuple.insert_boxed(build_partial(
                        field.type_info(),
                        field_value,
                        &format!("{path}.{name}.0"),
                    )?);
                    DynamicVariant::Tuple(tuple)
                }
                VariantType::Struct => {
                    let struct_info = variant
                        .as_struct_variant()
                        .map_err(|_| format!("{path}: variante '{name}' não tem campos"))?;
                    let mut dynamic = DynamicStruct::default();
                    if let Some(EcsValue::Map(fields)) = payload {
                        for (field_name, field_value) in fields {
                            if field_name == "variant" {
                                continue;
                            }
                            let field = struct_info.field(field_name).ok_or_else(|| {
                                format!("{path}.{name}: campo '{field_name}' não existe")
                            })?;
                            dynamic.insert_boxed(
                                field_name.clone(),
                                build_partial(
                                    field.type_info(),
                                    field_value,
                                    &format!("{path}.{name}.{field_name}"),
                                )?,
                            );
                        }
                    }
                    DynamicVariant::Struct(dynamic)
                }
            };
            let mut dynamic = DynamicEnum::new_with_index(index, name, dyn_variant);
            dynamic.set_represented_type(Some(info));
            Ok(Box::new(dynamic))
        }
        TypeInfo::TupleStruct(tuple_info) => {
            let values = match value {
                EcsValue::Seq(values) => values.clone(),
                _ => return Err(format!("{path}: esperava um array posicional")),
            };
            let mut dynamic = DynamicTupleStruct::default();
            dynamic.set_represented_type(Some(info));
            for (index, field_value) in values.iter().enumerate() {
                let field = tuple_info
                    .field_at(index)
                    .ok_or_else(|| format!("{path}: campo {index} não existe"))?;
                dynamic.insert_boxed(build_partial(
                    field.type_info(),
                    field_value,
                    &format!("{path}.{index}"),
                )?);
            }
            Ok(Box::new(dynamic))
        }
        _ => Err(format!(
            "{path}: tipo {} não construível por viber.ecs.insert",
            info.type_path()
        )),
    }
}

/// Payload de uma variante tuplo a partir da tabela `{variant="X", "0"=…}`.
fn field_from_variant_table(value: &EcsValue) -> Result<&EcsValue, String> {
    match value {
        EcsValue::Map(fields) => fields
            .iter()
            .find(|(name, _)| name == "0")
            .map(|(_, value)| value)
            .ok_or_else(|| "variante tuplo sem o campo \"0\"".to_string()),
        _ => Err("variante tuplo espera uma tabela {variant=…, \"0\"=…}".to_string()),
    }
}

/// Constrói um tipo "opaco" (primitivos e vetores da Bevy) a partir do valor.
fn build_opaque(
    info: &'static TypeInfo,
    value: &EcsValue,
    path: &str,
) -> Result<Box<dyn bevy::reflect::PartialReflect>, String> {
    use std::any::TypeId;
    let type_id = info.type_id();
    macro_rules! number {
        ($($ty:ty),* $(,)?) => {$(
            if type_id == TypeId::of::<$ty>() {
                let number = float_value(value, path)?;
                return Ok(Box::new(number as $ty));
            }
        )*};
    }
    macro_rules! integer {
        ($($ty:ty),* $(,)?) => {$(
            if type_id == TypeId::of::<$ty>() {
                let number = int_value(value, path)?;
                return Ok(Box::new(<$ty>::try_from(number).map_err(|_| {
                    format!("{path}: {number} fora do alcance de {}", stringify!($ty))
                })?));
            }
        )*};
    }
    if type_id == TypeId::of::<bool>() {
        return Ok(Box::new(match value {
            EcsValue::Bool(value) => *value,
            _ => return Err(format!("{path}: esperava um booleano")),
        }));
    }
    if type_id == TypeId::of::<String>() {
        return Ok(Box::new(match value {
            EcsValue::Str(value) => value.clone(),
            _ => return Err(format!("{path}: esperava uma string")),
        }));
    }
    integer!(i8, i16, i32, i64, isize);
    integer!(u8, u16, u32, u64, usize);
    number!(f32, f64);
    if type_id == TypeId::of::<Vec2>() {
        let [x, y] = vec_components::<2>(value, path)?;
        return Ok(Box::new(Vec2::new(x, y)));
    }
    if type_id == TypeId::of::<Vec3>() {
        let [x, y, z] = vec_components::<3>(value, path)?;
        return Ok(Box::new(Vec3::new(x, y, z)));
    }
    if type_id == TypeId::of::<Quat>() {
        let [x, y, z, w] = vec_components::<4>(value, path)?;
        return Ok(Box::new(Quat::from_xyzw(x, y, z, w)));
    }
    if type_id == TypeId::of::<Color>() {
        let [r, g, b, a] = color_components(value, path)?;
        return Ok(Box::new(Color::srgba(r, g, b, a)));
    }
    if type_id == TypeId::of::<Entity>() {
        let bits = int_value(value, path)?;
        let entity = Entity::try_from_bits(bits as u64)
            .ok_or_else(|| format!("{path}: id de entidade inválido ({bits})"))?;
        return Ok(Box::new(entity));
    }
    Err(format!(
        "{path}: tipo '{}' não construível por viber.ecs.insert",
        info.type_path()
    ))
}

// ------------------------------------------------------- atribuir um valor

/// Atribui `value` a um campo concreto, convertendo para o tipo do campo.
fn set_from_value(
    target: &mut dyn bevy::reflect::PartialReflect,
    value: &EcsValue,
    path: &str,
    registry: &TypeRegistry,
) -> Result<(), String> {
    macro_rules! integer {
        ($($ty:ty),* $(,)?) => {$(
            if let Some(slot) = target.try_downcast_mut::<$ty>() {
                let number = int_value(value, path)?;
                *slot = <$ty>::try_from(number).map_err(|_| {
                    format!("{path}: {number} fora do alcance de {}", stringify!($ty))
                })?;
                return Ok(());
            }
        )*};
    }
    macro_rules! unsigned {
        ($($ty:ty),* $(,)?) => {$(
            if let Some(slot) = target.try_downcast_mut::<$ty>() {
                let number = uint_value(value, path)?;
                *slot = <$ty>::try_from(number).map_err(|_| {
                    format!("{path}: {number} fora do alcance de {}", stringify!($ty))
                })?;
                return Ok(());
            }
        )*};
    }
    macro_rules! float {
        ($($ty:ty),* $(,)?) => {$(
            if let Some(slot) = target.try_downcast_mut::<$ty>() {
                *slot = float_value(value, path)? as $ty;
                return Ok(());
            }
        )*};
    }

    if let Some(slot) = target.try_downcast_mut::<bool>() {
        *slot = match value {
            EcsValue::Bool(value) => *value,
            _ => return Err(format!("{path}: esperava um booleano")),
        };
        return Ok(());
    }
    integer!(i8, i16, i32, i64, isize);
    unsigned!(u8, u16, u32, u64, usize);
    float!(f32, f64);
    if let Some(slot) = target.try_downcast_mut::<String>() {
        *slot = match value {
            EcsValue::Str(value) => value.clone(),
            _ => return Err(format!("{path}: esperava uma string")),
        };
        return Ok(());
    }
    if let Some(slot) = target.try_downcast_mut::<Vec2>() {
        let [x, y] = vec_components::<2>(value, path)?;
        *slot = Vec2::new(x, y);
        return Ok(());
    }
    if let Some(slot) = target.try_downcast_mut::<Vec3>() {
        let [x, y, z] = vec_components::<3>(value, path)?;
        *slot = Vec3::new(x, y, z);
        return Ok(());
    }
    if let Some(slot) = target.try_downcast_mut::<Quat>() {
        let [x, y, z, w] = vec_components::<4>(value, path)?;
        *slot = Quat::from_xyzw(x, y, z, w);
        return Ok(());
    }
    if let Some(slot) = target.try_downcast_mut::<Color>() {
        let [r, g, b, a] = color_components(value, path)?;
        *slot = Color::srgba(r, g, b, a);
        return Ok(());
    }
    if let Some(slot) = target.try_downcast_mut::<Entity>() {
        let bits = int_value(value, path)?;
        *slot = Entity::try_from_bits(bits as u64)
            .ok_or_else(|| format!("{path}: id de entidade inválido ({bits})"))?;
        return Ok(());
    }

    // Enums e structs aninhados.
    let type_path = target.reflect_type_path().to_string();
    let enum_info = target.get_represented_type_info().and_then(|info| {
        if let TypeInfo::Enum(enum_info) = info {
            Some(enum_info)
        } else {
            None
        }
    });
    if let Some(enum_info) = enum_info {
        return set_enum_from_value(target, enum_info, value, path, registry);
    }
    if target.reflect_mut().as_struct().is_ok() {
        match value {
            EcsValue::Map(_) | EcsValue::Unit | EcsValue::Nil => {
                return apply_patch_to_value(target, value, path, registry);
            }
            _ => {
                return Err(format!(
                    "{path}: '{type_path}' é um struct — esperava uma tabela de campos"
                ));
            }
        }
    }
    Err(format!(
        "{path}: tipo '{type_path}' não suportado por viber.ecs"
    ))
}

/// Atribui uma variante a um campo enum (`Visibility`, `Option`, `Faction`, …).
fn set_enum_from_value(
    target: &mut dyn bevy::reflect::PartialReflect,
    enum_info: &'static EnumInfo,
    value: &EcsValue,
    path: &str,
    registry: &TypeRegistry,
) -> Result<(), String> {
    let represented = target.get_represented_type_info();
    let current = target
        .reflect_ref()
        .as_enum()
        .map(Enum::variant_name)
        .unwrap_or_default()
        .to_string();

    // `Option<T>`: o `Some` atual recebe o valor; o `None` passa a `Some`
    // (payload construído do `TypeInfo` do campo 0); `nil`/`"none"` → `None`.
    if enum_info.variant("Some").is_some() && enum_info.variant("None").is_some() {
        if matches!(value, EcsValue::Nil)
            || matches!(value, EcsValue::Str(name) if name.eq_ignore_ascii_case("none"))
        {
            let none_index = enum_info.index_of("None").unwrap_or(0);
            let mut dynamic = DynamicEnum::new_with_index(none_index, "None", DynamicVariant::Unit);
            dynamic.set_represented_type(represented);
            return target
                .reflect_mut()
                .as_enum()
                .expect("enum")
                .try_apply(&dynamic)
                .map_err(|error| format!("{path}: {error}"));
        }
        if current == "Some" {
            let enum_mut = target.reflect_mut().as_enum().expect("enum");
            let field = enum_mut
                .field_at_mut(0)
                .ok_or_else(|| format!("{path}: Option::Some sem payload"))?;
            return set_from_value(field, value, path, registry);
        }
        let variant = enum_info
            .variant("Some")
            .ok_or_else(|| format!("{path}: variante Some ausente"))?;
        let tuple_info = variant
            .as_tuple_variant()
            .map_err(|_| format!("{path}: variante Some não é tuplo"))?;
        let field = tuple_info
            .field_at(0)
            .ok_or_else(|| format!("{path}: Some sem campo"))?;
        let payload = build_partial(field.type_info(), value, path)?;
        let mut tuple = DynamicTuple::default();
        tuple.insert_boxed(payload);
        let some_index = enum_info.index_of("Some").unwrap_or(0);
        let mut dynamic =
            DynamicEnum::new_with_index(some_index, "Some", DynamicVariant::Tuple(tuple));
        dynamic.set_represented_type(represented);
        return target
            .reflect_mut()
            .as_enum()
            .expect("enum")
            .try_apply(&dynamic)
            .map_err(|error| format!("{path}: {error}"));
    }

    let Some(name) = (match value {
        EcsValue::Str(name) => Some(name.as_str()),
        EcsValue::Enum(name) => Some(name.as_str()),
        _ => None,
    }) else {
        return Err(format!(
            "{path}: enum '{}' espera o nome da variante (string)",
            enum_info.type_path()
        ));
    };
    let variant = enum_info.variant(name).ok_or_else(|| {
        format!(
            "{path}: variante '{name}' não existe em {}",
            enum_info.type_path()
        )
    })?;
    let index = enum_info.index_of(name).unwrap_or(0);
    let dyn_variant = match variant.variant_type() {
        VariantType::Unit => DynamicVariant::Unit,
        _ => {
            return Err(format!(
                "{path}: variante '{name}' tem payload — passa uma tabela"
            ));
        }
    };
    let mut dynamic = DynamicEnum::new_with_index(index, name, dyn_variant);
    dynamic.set_represented_type(represented);
    target
        .reflect_mut()
        .as_enum()
        .expect("enum")
        .try_apply(&dynamic)
        .map_err(|error| format!("{path}: {error}"))
}

// --------------------------------------------------------------- conversões

fn float_value(value: &EcsValue, path: &str) -> Result<f64, String> {
    let number = match value {
        EcsValue::Float(value) => *value,
        EcsValue::Int(value) => *value as f64,
        EcsValue::UInt(value) => *value as f64,
        _ => {
            return Err(format!(
                "{path}: esperava um número (recebi {})",
                value_kind(value)
            ));
        }
    };
    if !number.is_finite() {
        return Err(format!("{path}: número não finito (NaN/inf)"));
    }
    Ok(number)
}

fn int_value(value: &EcsValue, path: &str) -> Result<i64, String> {
    match value {
        EcsValue::Int(value) => Ok(*value),
        EcsValue::Entity(bits) => Ok(*bits),
        EcsValue::UInt(value) => {
            i64::try_from(*value).map_err(|_| format!("{path}: {value} fora do alcance de i64"))
        }
        EcsValue::Float(value)
            if value.is_finite()
                && value.fract() == 0.0
                && *value >= i64::MIN as f64
                && *value <= i64::MAX as f64 =>
        {
            Ok(*value as i64)
        }
        _ => Err(format!(
            "{path}: esperava um inteiro (recebi {})",
            value_kind(value)
        )),
    }
}

fn uint_value(value: &EcsValue, path: &str) -> Result<u64, String> {
    match value {
        EcsValue::UInt(value) => Ok(*value),
        EcsValue::Entity(bits) if *bits >= 0 => Ok(*bits as u64),
        EcsValue::Int(value) if *value >= 0 => Ok(*value as u64),
        EcsValue::Float(value)
            if value.is_finite()
                && value.fract() == 0.0
                && *value >= 0.0
                && *value <= u64::MAX as f64 =>
        {
            Ok(*value as u64)
        }
        _ => Err(format!(
            "{path}: esperava um inteiro >= 0 (recebi {})",
            value_kind(value)
        )),
    }
}

fn vec_components<const N: usize>(value: &EcsValue, path: &str) -> Result<[f32; N], String> {
    let names: [&str; 4] = ["x", "y", "z", "w"];
    let mut out = [0.0f32; N];
    match value {
        EcsValue::Vec2(values) if N == 2 => {
            out.copy_from_slice(values);
            Ok(out)
        }
        EcsValue::Vec3(values) if N == 3 => {
            out.copy_from_slice(values);
            Ok(out)
        }
        EcsValue::Quat(values) if N == 4 => {
            out.copy_from_slice(values);
            Ok(out)
        }
        EcsValue::Seq(values) if values.len() == N => {
            for (index, item) in values.iter().enumerate() {
                out[index] = float_value(item, &format!("{path}[{}]", index + 1))? as f32;
            }
            Ok(out)
        }
        EcsValue::Map(fields) => {
            for index in 0..N {
                let name = names[index];
                let item = fields
                    .iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| value)
                    .ok_or_else(|| format!("{path}: falta '{name}'"))?;
                out[index] = float_value(item, &format!("{path}.{name}"))? as f32;
            }
            Ok(out)
        }
        _ => Err(format!(
            "{path}: esperava {{{},…}} ou um array de {N} números",
            names[..N].join(",")
        )),
    }
}

fn color_components(value: &EcsValue, path: &str) -> Result<[f32; 4], String> {
    match value {
        EcsValue::Color(values) => Ok(*values),
        EcsValue::Seq(values) if values.len() == 4 => {
            let mut out = [1.0f32; 4];
            for (index, item) in values.iter().enumerate() {
                out[index] = float_value(item, &format!("{path}[{}]", index + 1))? as f32;
            }
            Ok(out)
        }
        EcsValue::Map(fields) => {
            let mut out = [1.0f32; 4];
            let mut seen = [false; 3];
            for (key, item) in fields {
                let slot = match key.as_str() {
                    "r" => 0,
                    "g" => 1,
                    "b" => 2,
                    "a" => 3,
                    _ => continue,
                };
                out[slot] = float_value(item, &format!("{path}.{key}"))? as f32;
                if slot < 3 {
                    seen[slot] = true;
                }
            }
            if !(seen[0] && seen[1] && seen[2]) {
                return Err(format!("{path}: faltam canais r/g/b"));
            }
            Ok(out)
        }
        _ => Err(format!(
            "{path}: esperava {{r,g,b,a}} ou um array de 4 números"
        )),
    }
}

fn value_kind(value: &EcsValue) -> &'static str {
    match value {
        EcsValue::Unit => "tabela vazia",
        EcsValue::Nil => "nil",
        EcsValue::Bool(_) => "booleano",
        EcsValue::Int(_) | EcsValue::UInt(_) => "inteiro",
        EcsValue::Float(_) => "número",
        EcsValue::Str(_) | EcsValue::Enum(_) => "string",
        EcsValue::Vec2(_) | EcsValue::Vec3(_) | EcsValue::Quat(_) | EcsValue::Color(_) => "tabela",
        EcsValue::Entity(_) => "id de entidade",
        EcsValue::Map(_) | EcsValue::Seq(_) => "tabela",
    }
}
// --------------------------------------------------------------- testes
