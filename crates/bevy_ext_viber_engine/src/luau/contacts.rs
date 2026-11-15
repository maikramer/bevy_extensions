//! Contactos do Avian → hooks Luau.
//!
//! `CollisionStart`/`CollisionEnd` cobrem tudo o que interessa: o herói é um
//! corpo DINÂMICO, por isso os pares com estáticos/cinemáticos geram contacto
//! (no Rapier o KCC parava ~2 cm antes do overlap e os hits do controller
//! tinham de ser uma segunda fonte). A flag de sensor desapareceu do evento —
//! pergunta-se ao componente `Sensor` da entidade do collider.

use std::collections::HashMap;

use avian3d::prelude::{CollisionEnd, CollisionStart, Sensor};
use bevy::prelude::*;

use super::components::LuaScriptRef;
use super::host::{ContactHook, LuaScriptHost};

/// Par (dono do script, outra entidade) ainda em contacto.
#[derive(Resource, Default, Reflect)]
#[reflect(Resource)]
pub struct ContactBook {
    pairs: HashMap<(Entity, Entity), LivePair>,
}

#[derive(Clone, Copy, Reflect)]
struct LivePair {
    event: bool,
    trigger: bool,
}

impl LivePair {
    fn active(self) -> bool {
        self.event
    }
}

struct OtherBody {
    id: i64,
    name: Option<String>,
    pos: Vec3,
}

/// Depois do step do Rapier: eventos deste frame + hits do controller.
pub fn dispatch_contacts(
    // `Option`: apps de teste/headless que não trazem o `PhysicsPlugins` não
    // têm as mensagens de colisão inicializadas — o sistema não pode panicar.
    mut starts: Option<MessageReader<CollisionStart>>,
    mut ends: Option<MessageReader<CollisionEnd>>,
    time: Res<Time>,
    mut book: ResMut<ContactBook>,
    mut host: ResMut<LuaScriptHost>,
    scripts: Query<&LuaScriptRef>,
    names: Query<&Name>,
    props: Query<&crate::luau::ScriptProps>,
    globals: Query<&GlobalTransform>,
    parents: Query<&ChildOf>,
    sensors: Query<(), With<Sensor>>,
) {
    let dt = time.delta_secs();
    // Dono despawnado: o exit já não tem para quem correr. O OUTRO lado
    // também conta: um prop que desaparece a meio do contacto não gera
    // `CollisionEvent::Stopped` (o collider sai com a entidade) e o par
    // ficava vivo para sempre — `on_*_stay` corria todos os frames com um id
    // morto e a posição ZERO. Sem `GlobalTransform` a entidade não existe
    // para a física (Transform requer GlobalTransform), logo é o teste de
    // vivacidade certo aqui.
    //
    // O OUTRO lado despawnado ainda leva o hook de EXIT (sintético, só com o
    // id): para o Rapier o contacto acabou, e sem este evento um script que
    // conta quem está dentro do sensor nunca descia (o `Stopped` nunca chega).
    let mut fires: Vec<(Entity, ContactHook, OtherBody)> = Vec::new();
    let mut gone: Vec<((Entity, Entity), bool)> = Vec::new();
    book.pairs.retain(|key, live| {
        if scripts.get(key.0).is_err() {
            return false;
        }
        if globals.get(key.1).is_err() {
            gone.push((*key, live.trigger));
            return false;
        }
        true
    });
    for ((owner, other), trigger) in gone {
        fires.push((
            owner,
            ContactHook::exit(trigger),
            OtherBody {
                id: other.to_bits() as i64,
                name: None,
                pos: Vec3::ZERO,
            },
        ));
    }

    if let Some(starts) = starts.as_mut() {
        for event in starts.read() {
            let trigger = is_sensor(event.collider1, &sensors, &parents)
                || is_sensor(event.collider2, &sensors, &parents);
            note(
                &mut book,
                &scripts,
                &names,
                &globals,
                &parents,
                event.collider1,
                event.collider2,
                trigger,
                true,
                &mut fires,
            );
        }
    }
    if let Some(ends) = ends.as_mut() {
        for event in ends.read() {
            let trigger = is_sensor(event.collider1, &sensors, &parents)
                || is_sensor(event.collider2, &sensors, &parents);
            note(
                &mut book,
                &scripts,
                &names,
                &globals,
                &parents,
                event.collider1,
                event.collider2,
                trigger,
                false,
                &mut fires,
            );
        }
    }

    let stays: Vec<(Entity, OtherBody, bool)> = book
        .pairs
        .iter()
        .filter(|(_, live)| live.active())
        .map(|((owner, other), live)| {
            (
                *owner,
                describe(*other, &names, &globals, &parents),
                live.trigger,
            )
        })
        .collect();

    for (owner, hook, other) in fires {
        call(
            &mut host, &scripts, &names, &props, &globals, owner, hook, &other, dt,
        );
    }
    for (owner, other, trigger) in stays {
        call(
            &mut host,
            &scripts,
            &names,
            &props,
            &globals,
            owner,
            ContactHook::stay(trigger),
            &other,
            dt,
        );
    }
}

fn note(
    book: &mut ContactBook,
    scripts: &Query<&LuaScriptRef>,
    names: &Query<&Name>,
    globals: &Query<&GlobalTransform>,
    parents: &Query<&ChildOf>,
    a: Entity,
    b: Entity,
    trigger: bool,
    started: bool,
    fires: &mut Vec<(Entity, ContactHook, OtherBody)>,
) {
    let owner_a = script_owner(a, scripts, parents);
    let owner_b = script_owner(b, scripts, parents);
    if let Some(owner) = owner_a
        && owner != b
    {
        touch(
            book, scripts, names, globals, parents, owner, b, trigger, started, fires,
        );
    }
    if let Some(owner) = owner_b
        && owner != a
        && owner_a != Some(owner)
    {
        touch(
            book, scripts, names, globals, parents, owner, a, trigger, started, fires,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn touch(
    book: &mut ContactBook,
    scripts: &Query<&LuaScriptRef>,
    names: &Query<&Name>,
    globals: &Query<&GlobalTransform>,
    parents: &Query<&ChildOf>,
    owner: Entity,
    other: Entity,
    trigger: bool,
    started: bool,
    fires: &mut Vec<(Entity, ContactHook, OtherBody)>,
) {
    if scripts.get(owner).is_err() {
        return;
    }
    let other = subject(other, names, parents);
    if other == owner {
        return;
    }
    let key = (owner, other);
    let prev = book.pairs.get(&key).copied();
    let mut live = prev.unwrap_or(LivePair {
        event: false,
        trigger,
    });
    live.trigger = trigger;
    live.event = started;
    let was = prev.is_some_and(LivePair::active);
    let now = live.active();
    if now {
        book.pairs.insert(key, live);
    } else {
        book.pairs.remove(&key);
    }
    if was == now {
        return;
    }
    let hook = if now {
        ContactHook::enter(live.trigger)
    } else {
        ContactHook::exit(prev.map(|pair| pair.trigger).unwrap_or(live.trigger))
    };
    fires.push((owner, hook, describe(other, names, globals, parents)));
}

fn call(
    host: &mut LuaScriptHost,
    scripts: &Query<&LuaScriptRef>,
    names: &Query<&Name>,
    props: &Query<&crate::luau::ScriptProps>,
    globals: &Query<&GlobalTransform>,
    owner: Entity,
    hook: ContactHook,
    other: &OtherBody,
    dt: f32,
) {
    let Ok(script) = scripts.get(owner) else {
        return;
    };
    let origin = globals
        .get(owner)
        .map(|global| global.translation())
        .unwrap_or(Vec3::ZERO);
    if let Err(error) = host.activate_at(owner, &script.path, origin) {
        host.warn_once(&script.path, &error);
        return;
    }
    if let Err(error) = host.run_contact(
        owner,
        &script.path,
        origin,
        hook,
        other.id,
        other.name.as_deref(),
        other.pos,
        dt,
        names.get(owner).ok().map(Name::to_string),
        props.get(owner).ok().map(|props| props.0.clone()),
    ) {
        host.warn_once(&script.path, &error);
    }
}

fn script_owner(
    start: Entity,
    scripts: &Query<&LuaScriptRef>,
    parents: &Query<&ChildOf>,
) -> Option<Entity> {
    let mut current = start;
    loop {
        if scripts.get(current).is_ok() {
            return Some(current);
        }
        current = parents.get(current).ok()?.parent();
    }
}

fn is_sensor(start: Entity, sensors: &Query<(), With<Sensor>>, parents: &Query<&ChildOf>) -> bool {
    let mut current = start;
    loop {
        if sensors.get(current).is_ok() {
            return true;
        }
        match parents.get(current) {
            Ok(parent) => current = parent.parent(),
            Err(_) => return false,
        }
    }
}

/// O filho de offset chama-se `collider`; o hook vê o pai.
fn subject(entity: Entity, names: &Query<&Name>, parents: &Query<&ChildOf>) -> Entity {
    if names
        .get(entity)
        .is_ok_and(|name| name.as_str() == "collider")
        && let Ok(parent) = parents.get(entity)
    {
        return parent.parent();
    }
    entity
}

fn describe(
    entity: Entity,
    names: &Query<&Name>,
    globals: &Query<&GlobalTransform>,
    parents: &Query<&ChildOf>,
) -> OtherBody {
    let entity = subject(entity, names, parents);
    OtherBody {
        id: entity.to_bits() as i64,
        name: names.get(entity).map(|name| name.as_str().to_string()).ok(),
        pos: globals
            .get(entity)
            .map(|global| global.translation())
            .unwrap_or(Vec3::ZERO),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::stamp_collider_role;
    use avian3d::prelude::*;
    use bevy::time::{TimePlugin, TimeUpdateStrategy};
    use std::time::Duration;

    fn physics_app(code: &str) -> App {
        let mut host = LuaScriptHost::new(std::path::PathBuf::from("scripts")).expect("host");
        host.load_script("zone.lua", code).expect("script");
        let mut app = App::new();
        app.add_plugins((
            TransformPlugin,
            TimePlugin,
            bevy::asset::AssetPlugin::default(),
            PhysicsPlugins::default(),
        ));
        app.init_asset::<Mesh>();
        app.add_message::<bevy::asset::AssetEvent<Mesh>>();
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(
            1.0 / 60.0,
        )));
        app.insert_resource(host);
        app.init_resource::<ContactBook>();
        app.add_systems(PostUpdate, dispatch_contacts);
        // `App::update` não corre o `finish` dos plugins; sem ele o
        // `SpatialQueryDiagnostics` não existe e os sistemas de query panicam.
        app.finish();
        app
    }

    fn lua_count(app: &App, key: &str) -> i64 {
        let host = app.world().resource::<LuaScriptHost>();
        match host.script_global("zone.lua", key).expect("global") {
            mlua::Value::Integer(n) => n,
            mlua::Value::Number(n) => n as i64,
            mlua::Value::Nil => 0,
            other => panic!("{key} = {other:?}"),
        }
    }

    fn lua_string(app: &App, key: &str) -> String {
        let host = app.world().resource::<LuaScriptHost>();
        match host.script_global("zone.lua", key).expect("global") {
            mlua::Value::String(text) => text.to_str().map(|s| s.to_string()).unwrap_or_default(),
            mlua::Value::Nil => String::new(),
            other => panic!("{key} = {other:?}"),
        }
    }

    #[test]
    fn trigger_enter_and_exit_reach_the_script() {
        let mut app = physics_app(
            "entered = 0\n\
             exited = 0\n\
             last = ''\n\
             function on_trigger_enter(other)\n\
               entered = entered + 1\n\
               last = other.name or ''\n\
             end\n\
             function on_trigger_exit(other)\n\
               exited = exited + 1\n\
             end\n",
        );
        let zone = app
            .world_mut()
            .spawn((
                Transform::default(),
                RigidBody::Static,
                LuaScriptRef {
                    path: "zone.lua".into(),
                },
                Name::new("zone"),
            ))
            .id();
        {
            // O papel ANTES do collider (o Avian lê a flag no proxy).
            let mut entity = app.world_mut().entity_mut(zone);
            stamp_collider_role(&mut entity, true, true);
            entity.insert(Collider::cuboid(2.0, 2.0, 2.0));
        }
        assert!(app.world().get::<Sensor>(zone).is_some());
        let hero = app
            .world_mut()
            .spawn((
                Transform::from_xyz(6.0, 0.0, 0.0),
                RigidBody::Dynamic,
                GravityScale(0.0),
                Collider::sphere(0.4),
                Name::new("hero"),
            ))
            .id();
        for _ in 0..5 {
            app.update();
        }
        assert_eq!(lua_count(&app, "entered"), 0, "ainda longe");
        app.world_mut()
            .get_mut::<Transform>(hero)
            .unwrap()
            .translation = Vec3::ZERO;
        for _ in 0..12 {
            app.update();
        }
        assert_eq!(lua_count(&app, "entered"), 1, "um enter");
        assert_eq!(lua_string(&app, "last"), "hero");
        app.world_mut()
            .get_mut::<Transform>(hero)
            .unwrap()
            .translation = Vec3::new(6.0, 0.0, 0.0);
        for _ in 0..12 {
            app.update();
        }
        assert_eq!(lua_count(&app, "exited"), 1, "um exit");
    }

    #[test]
    fn dynamic_body_hit_is_one_collision_enter() {
        let mut app = physics_app(
            "entered = 0\n\
             function on_collision_enter(other)\n\
               entered = entered + 1\n\
               last = other.name or ''\n\
             end\n",
        );
        let wall = app
            .world_mut()
            .spawn((
                Transform::default(),
                RigidBody::Static,
                LuaScriptRef {
                    path: "zone.lua".into(),
                },
            ))
            .id();
        {
            // O papel ANTES do collider (o Avian lê a flag no proxy).
            let mut entity = app.world_mut().entity_mut(wall);
            stamp_collider_role(&mut entity, false, true);
            entity.insert(Collider::cuboid(1.0, 2.0, 2.0));
        }
        assert!(app.world().get::<Sensor>(wall).is_none());
        // O herói do jogo é DINÂMICO (tnua): o contacto chega como
        // `CollisionStart`, uma única vez, sem a segunda fonte do KCC.
        app.world_mut().spawn((
            Transform::from_xyz(2.0, 0.0, 0.0),
            RigidBody::Dynamic,
            Collider::sphere(0.35),
            GravityScale(0.0),
            LinearVelocity(Vec3::new(-20.0, 0.0, 0.0)),
            Name::new("hero"),
        ));
        for _ in 0..20 {
            app.update();
        }
        assert_eq!(lua_count(&app, "entered"), 1, "um enter");
        assert_eq!(lua_string(&app, "last"), "hero");
    }

    #[test]
    fn other_despawned_mid_contact_fires_exit() {
        // Um corpo que desaparece DENTRO do sensor não gera
        // `CollisionEvent::Stopped`: sem um exit sintético o script que conta
        // quem está dentro ficava com a contagem presa para sempre.
        let mut app = physics_app(
            "entered = 0\n\
             exited = 0\n\
             function on_trigger_enter(other)\n\
               entered = entered + 1\n\
             end\n\
             function on_trigger_exit(other)\n\
               exited = exited + 1\n\
             end\n",
        );
        let zone = app
            .world_mut()
            .spawn((
                Transform::default(),
                RigidBody::Static,
                LuaScriptRef {
                    path: "zone.lua".into(),
                },
                Name::new("zone"),
            ))
            .id();
        {
            // O papel ANTES do collider (o Avian lê a flag no proxy).
            let mut entity = app.world_mut().entity_mut(zone);
            stamp_collider_role(&mut entity, true, true);
            entity.insert(Collider::cuboid(2.0, 2.0, 2.0));
        }
        let hero = app
            .world_mut()
            .spawn((
                Transform::from_xyz(6.0, 0.0, 0.0),
                RigidBody::Dynamic,
                GravityScale(0.0),
                Collider::sphere(0.4),
                Name::new("hero"),
            ))
            .id();
        for _ in 0..5 {
            app.update();
        }
        app.world_mut()
            .get_mut::<Transform>(hero)
            .unwrap()
            .translation = Vec3::ZERO;
        for _ in 0..12 {
            app.update();
        }
        assert_eq!(lua_count(&app, "entered"), 1, "um enter");
        // Despawn a meio do contacto: o Rapier não emite Stopped.
        app.world_mut().despawn(hero);
        for _ in 0..5 {
            app.update();
        }
        assert_eq!(lua_count(&app, "exited"), 1, "exit sintético uma vez");
    }
}
