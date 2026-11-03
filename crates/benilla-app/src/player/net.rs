//! The player's packet handlers: `SMSG_STANDSTATE_UPDATE` goes to [`super::posture`] as a
//! [`ServerStandState`], applied in `control`, which holds the body query and sheath queue;
//! `SMSG_TIME_SYNC_REQ` is answered here.

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{Player, ServerStandState};
use crate::creature_anim::move_flags;
use crate::entities::CollisionHeight;
use crate::net::{ClientCommand, NetCommands, NetHandlerApp, SelfGuid, SelfPlayer};

/// Registers the handlers; called from [`super::PlayerPlugin`].
pub(super) fn register(app: &mut App) {
    app.net_handler(SessionEventKind::StandStateUpdate, on_stand_state_update)
        .net_handler(SessionEventKind::TimeSyncReq, on_time_sync_req)
        .net_handler(
            SessionEventKind::MoveSetCollisionHeight,
            on_set_collision_height,
        )
        .net_handler(SessionEventKind::ClearFarSightImmediate, on_clear_far_sight);
}

/// `SMSG_CLEAR_FAR_SIGHT_IMMEDIATE` (`0x6e2b00`): with our player held and far sight on (its
/// subject resolved), far sight goes off (`0x6e2b0e`) and the camera snaps back to the body
/// (`0x6e2b26`) rather than gliding there, without waiting for `PLAYER_FARSIGHT` to clear.
fn on_clear_far_sight(
    In(ev): In<SessionEvent>,
    me: Query<&crate::net::ObjectStore, With<SelfPlayer>>,
    index: Res<crate::net::GuidIndex>,
    mut cleared: ResMut<super::view_subject::FarSightCleared>,
    rig: Option<ResMut<super::camera::CameraControl>>,
) {
    if !matches!(ev, SessionEvent::ClearFarSightImmediate) {
        return;
    }
    let Some(guid) = me.iter().next().and_then(|store| store.0.player_farsight()) else {
        return;
    };
    if cleared.0 == Some(guid) || !index.0.contains_key(&guid) {
        return;
    }
    cleared.0 = Some(guid);
    if let Some(mut rig) = rig {
        rig.pivot.snap_next();
    }
}

/// `SMSG_TIME_SYNC_REQ` (`0x6dc010`): with our player held, the counter and the movement clock
/// at handling (`0x6ef370`) go back as `CMSG_TIME_SYNC_RESP` (`0x6e9bb0`). The reference queues
/// the two as a move event its next movement update sends; benilla has no move queue, and a
/// request's event is due on arrival, so the answer goes at once with the same stamp. Not applied:
/// the queue's arm (`0x6ef860`) first drops move flag `0x200` when set, which no live run has
/// tied to a state benilla's mover holds.
fn on_time_sync_req(
    In(ev): In<SessionEvent>,
    me: Query<(), With<SelfPlayer>>,
    commands: Res<NetCommands>,
) {
    let SessionEvent::TimeSyncReq { counter } = ev else {
        return;
    };
    if me.is_empty() {
        return;
    }
    let client_ms = benilla_protocol::client_uptime_ms();
    super::move_trace::time_sync(counter, client_ms);
    let _ = commands
        .0
        .send(ClientCommand::TimeSyncResp { counter, client_ms });
}

/// `SMSG_MOVE_SET_COLLISION_HGT` on our mover (`0x7307a0`, then `0x6e9600` at the event): the
/// body's collision height becomes the server's until the model sets it again, and the ack
/// carries our pose and the height. The depth lines read it through [`Player::collision_height`].
fn on_set_collision_height(
    In(ev): In<SessionEvent>,
    self_guid: Res<SelfGuid>,
    player: Option<Res<Player>>,
    mut body: Query<&mut CollisionHeight, With<SelfPlayer>>,
    commands: Res<NetCommands>,
) {
    let SessionEvent::MoveSetCollisionHeight {
        guid,
        counter,
        height,
    } = ev
    else {
        return;
    };
    // A build with no mover has no body to resize.
    let Some(player) = player.filter(|_| self_guid.0 == Some(guid)) else {
        return;
    };
    if let Ok(mut h) = body.single_mut() {
        h.0 = height;
    }
    let flying = if player.flying { move_flags::FLYING } else { 0 };
    let _ = commands.0.send(ClientCommand::CollisionHeightAck {
        guid,
        counter,
        height,
        flags: player.modes.wire_flags() | flying,
        pos: benilla_assets::coords::bevy_to_wow(player.pos),
        orientation: player.face_yaw.rem_euclid(std::f32::consts::TAU),
    });
}

fn on_stand_state_update(In(ev): In<SessionEvent>, mut out: MessageWriter<ServerStandState>) {
    if let SessionEvent::StandStateUpdate { state } = ev {
        out.write(ServerStandState { state });
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::net::{GuidIndex, ObjectStore};
    use crate::player::view_subject::FarSightCleared;

    /// A world holding our player, whose `PLAYER_FARSIGHT` names `guid`, streamed or not.
    fn world_viewing(guid: u32, streamed: bool) -> World {
        const FARSIGHT: u16 = 712;
        let mut world = World::new();
        world.init_resource::<FarSightCleared>();
        world.init_resource::<GuidIndex>();
        world.spawn((
            SelfPlayer,
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (FARSIGHT, guid),
                (FARSIGHT + 1, 0),
            ])),
        ));
        if streamed {
            let subject = world.spawn_empty().id();
            world
                .resource_mut::<GuidIndex>()
                .0
                .insert(u64::from(guid), subject);
        }
        world
    }

    fn clear(world: &mut World) -> Option<u64> {
        world
            .run_system_once_with(on_clear_far_sight, SessionEvent::ClearFarSightImmediate)
            .expect("the handler runs as a one-shot system");
        world.resource::<FarSightCleared>().0
    }

    /// The clear turns off a far sight that is on: our field names a streamed subject.
    #[test]
    fn the_clear_ends_a_resolved_far_sight() {
        assert_eq!(clear(&mut world_viewing(0x1234, true)), Some(0x1234));
    }

    /// With no far sight on, or one whose subject has not streamed, there is nothing to end.
    #[test]
    fn the_clear_without_a_far_sight_does_nothing() {
        assert_eq!(clear(&mut world_viewing(0, true)), None);
        assert_eq!(clear(&mut world_viewing(0x1234, false)), None);
    }
}
