use crate::Vec2;
use serde::{Deserialize, Serialize};

/// Lo que el cliente envía cada tick. Mismos campos que CNetObj_PlayerInput.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Input {
    /// -1 izquierda, 0 quieto, 1 derecha
    pub dir: i8,
    pub jump: bool,
    pub hook: bool,
    pub fire: bool,
    /// Dirección de aim normalizada (hacia el cursor)
    pub aim: Vec2,
    /// 0 = sin cambio; 1..=5 = seleccionar arma (Hammer..Laser)
    pub weapon: u8,
}
