// player_movement_tick_with_fluids' HEAD: a spectator the server lets neither fly nor start flying.
fn backcompat_spectator_note(world: &mut MotionWorld<'_, '_, '_>) {
    let p = &world.player;
    crate::net::backcompat::note_grounded_spectator(p.spectator && !p.may_fly && !p.flying);
}
