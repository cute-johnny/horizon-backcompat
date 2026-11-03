// dry_tick's HEAD: the tick forced that spectator's flight; vanilla's travel would let it fall.
fn backcompat_spectator_travel(world: &mut MotionWorld<'_, '_, '_>) {
    if crate::net::backcompat::take_grounded_spectator() && world.player.spectator {
        world.player.flying = false;
    }
}
