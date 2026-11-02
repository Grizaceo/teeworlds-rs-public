//! Gym de RL sobre el sim compartido: episodios deterministas con N seats
//! (cerebros, bots scripteados, "humano" simulado), recompensas por especie
//! con topes y métricas/alarmas de reward hacking. Sin deps externas;
//! el paralelismo usa `std::thread::scope`.

pub mod config;
pub mod env;
pub mod metrics;
pub mod reward;
pub mod scripted;
pub mod report;
