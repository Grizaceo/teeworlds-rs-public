//! Términos de recompensa del modo Estómago:
//! - `eat` paga al captor cuando emite `Event::Eaten`,
//! - `escape` paga al comido cuando lo eyectan (`Event::Ejected`),
//! - `big_survival` paga por sobrevivir con size > base.

use gym::config::species_rewards;
use gym::reward::RewardTracker;
use shared::creature::Species;
use shared::{GameMode, Input, Map, Vec2, World};

#[test]
fn stomach_eat_escape_and_big_survival() {
    let mut w = World::new(Map::test_map()).with_mode(GameMode::Stomach);
    w.spawn_protect_ticks = 0;
    w.add_player(1, "captor".into());
    w.add_player(2, "presa".into());
    w.add_player(3, "verdugo".into());
    for (id, x) in [(1u32, 200.0f32), (2, 400.0), (3, 900.0)] {
        w.players.get_mut(&id).unwrap().core.pos = Vec2::new(x, 800.0);
    }
    // Captor ya grande: big_survival debe pagar desde el primer tick vivo.
    w.players.get_mut(&1).unwrap().core.size = 1.6;
    let mut rt1 = RewardTracker::new(species_rewards(Species::Tanque));
    let mut rt2 = RewardTracker::new(species_rewards(Species::Evasiva));

    // 1 le dispara a 2 hasta comérselo (pistola de spawn, misma receta
    // que el test de shared: aim (1,0) a lo largo del suelo plano).
    w.set_input(
        1,
        Input {
            fire: true,
            aim: Vec2::new(1.0, 0.0),
            ..Default::default()
        },
    );
    let mut eaten = false;
    for _ in 0..2000 {
        w.tick();
        rt1.update(&w, 1, &[]);
        rt2.update(&w, 2, &[]);
        if w.players.get(&2).map(|p| p.eaten_by) == Some(Some(1)) {
            eaten = true;
            break;
        }
    }
    assert!(eaten, "2 no quedó comido por 1 en 2000 ticks");
    assert!(
        rt1.terms().eat > 0.0,
        "Event::Eaten no pagó `eat`: {:?}",
        rt1.terms().named()
    );
    assert!(
        rt1.terms().big_survival > 0.0,
        "size>base no pagó `big_survival`: {:?}",
        rt1.terms().named()
    );

    // El verdugo mata al captor → la presa es eyectada → `escape`.
    w.set_input(1, Input::default());
    w.players.get_mut(&1).unwrap().health = 1;
    w.players.get_mut(&1).unwrap().armor = 0;
    w.players.get_mut(&3).unwrap().core.pos = Vec2::new(300.0, 800.0);
    w.set_input(
        3,
        Input {
            fire: true,
            aim: Vec2::new(-1.0, 0.0),
            ..Default::default()
        },
    );
    let mut escaped = false;
    for _ in 0..2000 {
        w.tick();
        rt2.update(&w, 2, &[]);
        if w.players
            .get(&2)
            .map(|p| p.alive && p.eaten_by.is_none())
            .unwrap_or(false)
        {
            escaped = true;
            break;
        }
    }
    assert!(escaped, "2 no fue eyectada tras morir el captor");
    assert!(
        rt2.terms().escape > 0.0,
        "Event::Ejected no pagó `escape`: {:?}",
        rt2.terms().named()
    );
}
