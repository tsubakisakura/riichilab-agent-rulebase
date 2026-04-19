use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Seat(u8);

impl Seat {
    pub const fn new(value: u8) -> Result<Self, ProtocolError> {
        if value <= 3 { Ok(Self(value)) } else { Err(ProtocolError::InvalidSeat(value)) }
    }

    pub const fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for Seat {
    type Error = ProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl Serialize for Seat {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u8(self.0)
    }
}

impl<'de> Deserialize<'de> for Seat {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u8::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wind {
    East,
    South,
    West,
    North,
}

impl Wind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::East => "E",
            Self::South => "S",
            Self::West => "W",
            Self::North => "N",
        }
    }
}

impl Serialize for Wind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Wind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct WindVisitor;

        impl Visitor<'_> for WindVisitor {
            type Value = Wind;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(r#""E", "S", "W", or "N""#)
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                match value {
                    "E" => Ok(Wind::East),
                    "S" => Ok(Wind::South),
                    "W" => Ok(Wind::West),
                    "N" => Ok(Wind::North),
                    _ => Err(E::custom(format!("invalid wind: {value}"))),
                }
            }
        }

        deserializer.deserialize_str(WindVisitor)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dragon {
    Haku,
    Hatsu,
    Chun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Honor {
    Wind(Wind),
    Dragon(Dragon),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suit {
    Man,
    Pin,
    Sou,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tile {
    Suited { suit: Suit, rank: u8, red: bool },
    Honor(Honor),
}

impl Tile {
    pub fn is_yaochuuhai(self) -> bool {
        match self {
            Self::Suited { rank, .. } => rank == 1 || rank == 9,
            Self::Honor(_) => true,
        }
    }
}

impl fmt::Display for Tile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Suited { suit, rank, red } => {
                let suit = match suit {
                    Suit::Man => 'm',
                    Suit::Pin => 'p',
                    Suit::Sou => 's',
                };
                write!(f, "{rank}{suit}")?;
                if *red {
                    write!(f, "r")?;
                }
                Ok(())
            }
            Self::Honor(Honor::Wind(wind)) => f.write_str(wind.as_str()),
            Self::Honor(Honor::Dragon(dragon)) => f.write_str(match dragon {
                Dragon::Haku => "P",
                Dragon::Hatsu => "F",
                Dragon::Chun => "C",
            }),
        }
    }
}

impl FromStr for Tile {
    type Err = ProtocolError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "E" => return Ok(Self::Honor(Honor::Wind(Wind::East))),
            "S" => return Ok(Self::Honor(Honor::Wind(Wind::South))),
            "W" => return Ok(Self::Honor(Honor::Wind(Wind::West))),
            "N" => return Ok(Self::Honor(Honor::Wind(Wind::North))),
            "P" => return Ok(Self::Honor(Honor::Dragon(Dragon::Haku))),
            "F" => return Ok(Self::Honor(Honor::Dragon(Dragon::Hatsu))),
            "C" => return Ok(Self::Honor(Honor::Dragon(Dragon::Chun))),
            _ => {}
        }

        let bytes = value.as_bytes();
        if !(bytes.len() == 2 || bytes.len() == 3) {
            return Err(ProtocolError::InvalidTile(value.to_owned()));
        }

        let rank = match bytes[0] {
            b'1'..=b'9' => bytes[0] - b'0',
            _ => return Err(ProtocolError::InvalidTile(value.to_owned())),
        };
        let suit = match bytes[1] {
            b'm' => Suit::Man,
            b'p' => Suit::Pin,
            b's' => Suit::Sou,
            _ => return Err(ProtocolError::InvalidTile(value.to_owned())),
        };
        let red = match bytes.get(2) {
            None => false,
            Some(b'r') if rank == 5 => true,
            _ => return Err(ProtocolError::InvalidTile(value.to_owned())),
        };

        Ok(Self::Suited { suit, rank, red })
    }
}

impl Serialize for Tile {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Tile {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaybeMaskedTile {
    Known(Tile),
    Unknown,
}

impl Serialize for MaybeMaskedTile {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Known(tile) => tile.serialize(serializer),
            Self::Unknown => serializer.serialize_str("?"),
        }
    }
}

impl<'de> Deserialize<'de> for MaybeMaskedTile {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if value == "?" { Ok(Self::Unknown) } else { value.parse().map(Self::Known).map_err(de::Error::custom) }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Dahai {
        actor: Seat,
        pai: Tile,
        #[serde(default)]
        tsumogiri: bool,
    },
    Chi {
        actor: Seat,
        target: Seat,
        pai: Tile,
        consumed: Vec<Tile>,
    },
    Pon {
        actor: Seat,
        target: Seat,
        pai: Tile,
        consumed: Vec<Tile>,
    },
    Daiminkan {
        actor: Seat,
        target: Seat,
        pai: Tile,
        consumed: Vec<Tile>,
    },
    Ankan {
        actor: Seat,
        consumed: Vec<Tile>,
    },
    Kakan {
        actor: Seat,
        pai: Tile,
        consumed: Vec<Tile>,
    },
    Reach {
        actor: Seat,
    },
    Hora {
        actor: Seat,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<Seat>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pai: Option<Tile>,
    },
    Ryukyoku,
    None,
}

pub type ExtraFields = BTreeMap<String, Value>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    StartGame {
        id: Seat,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    StartKyoku {
        bakaze: Wind,
        kyoku: u8,
        honba: u8,
        kyotaku: u8,
        oya: Seat,
        dora_marker: Tile,
        tehais: Vec<Vec<MaybeMaskedTile>>,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    Tsumo {
        actor: Seat,
        pai: MaybeMaskedTile,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    Dahai {
        actor: Seat,
        pai: Tile,
        #[serde(default)]
        tsumogiri: bool,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    Chi {
        actor: Seat,
        target: Seat,
        pai: Tile,
        consumed: Vec<Tile>,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    Pon {
        actor: Seat,
        target: Seat,
        pai: Tile,
        consumed: Vec<Tile>,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    Daiminkan {
        actor: Seat,
        target: Seat,
        pai: Tile,
        consumed: Vec<Tile>,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    Ankan {
        actor: Seat,
        consumed: Vec<Tile>,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    Kakan {
        actor: Seat,
        pai: Tile,
        consumed: Vec<Tile>,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    Reach {
        actor: Seat,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    Hora {
        actor: Seat,
        #[serde(default)]
        target: Option<Seat>,
        #[serde(default)]
        pai: Option<Tile>,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    EndKyoku {
        #[serde(flatten)]
        extra: ExtraFields,
    },
    EndGame {
        scores: Vec<i32>,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    RequestAction {
        possible_actions: Vec<Action>,
        observation: String,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    ValidationResult {
        passed: bool,
        #[serde(default)]
        reason: Option<String>,
        #[serde(flatten)]
        extra: ExtraFields,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    InvalidSeat(u8),
    InvalidTile(String),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSeat(value) => write!(f, "invalid seat: {value}"),
            Self::InvalidTile(value) => write!(f, "invalid tile: {value}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_round_trip() {
        let cases = ["1m", "5pr", "9s", "E", "P", "C"];
        for case in cases {
            let tile: Tile = case.parse().unwrap();
            assert_eq!(tile.to_string(), case);
        }
    }

    #[test]
    fn masked_tile_deserializes_unknown() {
        let tile: MaybeMaskedTile = serde_json::from_str("\"?\"").unwrap();
        assert_eq!(tile, MaybeMaskedTile::Unknown);
    }

    #[test]
    fn request_action_deserializes() {
        let json = r#"{
            "type": "request_action",
            "possible_actions": [
                {"type":"dahai","actor":0,"pai":"3m","tsumogiri":true},
                {"type":"none"}
            ],
            "observation": "ZmFrZQ=="
        }"#;

        let event: Event = serde_json::from_str(json).unwrap();
        match event {
            Event::RequestAction { possible_actions, observation, extra } => {
                assert_eq!(possible_actions.len(), 2);
                assert_eq!(observation, "ZmFrZQ==");
                assert!(extra.is_empty());
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn start_kyoku_deserializes_masked_tiles() {
        let json = r#"{
            "type": "start_kyoku",
            "bakaze": "E",
            "kyoku": 1,
            "honba": 0,
            "kyotaku": 0,
            "oya": 0,
            "dora_marker": "2p",
            "tehais": [
                ["1m","2m","3m","4m","5m","6m","7m","8m","9m","E","S","W","N"],
                ["?","?","?","?","?","?","?","?","?","?","?","?","?"],
                ["?","?","?","?","?","?","?","?","?","?","?","?","?"],
                ["?","?","?","?","?","?","?","?","?","?","?","?","?"]
            ]
        }"#;

        let event: Event = serde_json::from_str(json).unwrap();
        match event {
            Event::StartKyoku { tehais, .. } => {
                assert!(matches!(tehais[1][0], MaybeMaskedTile::Unknown));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn seat_rejects_out_of_range() {
        let err = serde_json::from_str::<Seat>("4").unwrap_err();
        assert!(err.to_string().contains("invalid seat"));
    }
}
