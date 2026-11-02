use crate::Vec2;
use serde::{Deserialize, Serialize};

/// Lo que el cliente envía cada tick. Mismos campos que CNetObj_PlayerInput.
/// Campos nuevos SIEMPRE al final (compatibilidad del protocolo).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
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
    /// Mantener = dash (Shift por defecto); la sim detecta el flanco.
    pub dash: bool,
    /// Mantener = parry (Q por defecto); la sim detecta el flanco.
    pub parry: bool,
    /// Preferencia del planeador del jugador:
    /// 0 = solo tras gastar los saltos aéreos (default),
    /// 1 = siempre que se caiga con `jump` mantenido.
    pub glide: u8,
}
