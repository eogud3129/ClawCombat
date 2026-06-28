use std::{fmt::Display, str::FromStr};

use crate::{game::posture::Posture, types::Coverage};

#[derive(Clone, Debug)]
pub enum TileType {
    ShortGrass,
    MiddleGrass,
    HighGrass,
    Dirt,
    Mud,
    Concrete,
    BrickWall,
    Trunk,
    Water,
    DeepWater,
    Underbrush,
    LightUnderbrush,
    MiddleWoodLogs,
    Hedge,
    MiddleRock,
}

impl FromStr for TileType {
    type Err = TerrainTileError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ShortGrass" => Ok(Self::ShortGrass),
            "MiddleGrass" => Ok(Self::MiddleGrass),
            "HighGrass" => Ok(Self::HighGrass),
            "Dirt" => Ok(Self::Dirt),
            "Mud" => Ok(Self::Mud),
            "Concrete" => Ok(Self::Concrete),
            "BrickWall" => Ok(Self::BrickWall),
            "Trunk" => Ok(Self::Trunk),
            "Water" => Ok(Self::Water),
            "DeepWater" => Ok(Self::DeepWater),
            "Underbrush" => Ok(Self::Underbrush),
            "LightUnderbrush" => Ok(Self::LightUnderbrush),
            "MiddleWoodLogs" => Ok(Self::MiddleWoodLogs),
            "Hedge" => Ok(Self::Hedge),
            "MiddleRock" => Ok(Self::MiddleRock),
            _ => Result::Err(TerrainTileError::UnknownId(s.to_string())),
        }
    }
}

impl TileType {
    pub fn pedestrian_cost(&self) -> i32 {
        match self {
            // [비용 현실화] 개활지와 맨땅의 비용을 정상화하여 무리한 외곽 우회 기동을 방지합니다.
            TileType::ShortGrass => 20,
            TileType::MiddleGrass => 18,
            TileType::HighGrass => 15,
            TileType::Dirt => 20,
            TileType::Mud => 30,
            TileType::Concrete => 20,
            TileType::Water => 50,
            TileType::DeepWater => 200,

            // 엄폐물 선호 기조는 유지하되 갭을 줄입니다.
            TileType::Underbrush => 10,
            TileType::LightUnderbrush => 12,
            TileType::Hedge => 15,
            TileType::MiddleWoodLogs => 15,
            TileType::MiddleRock => 15,

            // 벽, 나무기둥 자체는 관통 불가 (강력한 페널티 유지)
            TileType::BrickWall => 500,
            TileType::Trunk => 500,
        }
    }

    pub fn block_vehicle(&self) -> bool {
        match self {
            TileType::ShortGrass
            | TileType::MiddleGrass
            | TileType::HighGrass
            | TileType::Dirt
            | TileType::Mud
            | TileType::Concrete
            | TileType::Water
            | TileType::Underbrush
            | TileType::LightUnderbrush
            | TileType::MiddleWoodLogs
            | TileType::Hedge => false,
            TileType::BrickWall | TileType::Trunk | TileType::DeepWater | TileType::MiddleRock => {
                true
            }
        }
    }

    pub fn coverage(&self, posture: &Posture) -> Option<Coverage> {
        match posture {
            Posture::StandUp => match self {
                TileType::ShortGrass => None,
                TileType::MiddleGrass => None,
                TileType::HighGrass => None,
                TileType::Dirt => None,
                TileType::Mud => None,
                TileType::Concrete => None,
                TileType::BrickWall => Some(Coverage(0.8)),
                TileType::Trunk => Some(Coverage(0.9)),
                TileType::Water => None,
                TileType::DeepWater => None,
                TileType::Underbrush => None,
                TileType::LightUnderbrush => None,
                TileType::MiddleWoodLogs => Some(Coverage(0.2)),
                TileType::Hedge => Some(Coverage(0.15)),
                TileType::MiddleRock => Some(Coverage(0.2)),
            },
            Posture::Flat => match self {
                TileType::ShortGrass => None,
                TileType::MiddleGrass => None,
                TileType::HighGrass => None,
                TileType::Dirt => None,
                TileType::Mud => Some(Coverage(0.3)),
                TileType::Concrete => None,
                TileType::BrickWall => Some(Coverage(0.8)),
                TileType::Trunk => Some(Coverage(0.7)),
                TileType::Water => None,
                TileType::DeepWater => None,
                TileType::Underbrush => None,
                TileType::LightUnderbrush => None,
                TileType::MiddleWoodLogs => Some(Coverage(0.7)),
                TileType::Hedge => Some(Coverage(0.15)),
                TileType::MiddleRock => Some(Coverage(0.75)),
            },
        }
    }

    pub fn block_bullet(&self) -> bool {
        match self {
            TileType::ShortGrass => false,
            TileType::MiddleGrass => false,
            TileType::HighGrass => false,
            TileType::Dirt => false,
            TileType::Mud => false,
            TileType::Concrete => false,
            TileType::BrickWall => true,
            TileType::Trunk => true,
            TileType::Water => false,
            TileType::DeepWater => false,
            TileType::Underbrush => false,
            TileType::LightUnderbrush => false,
            TileType::MiddleWoodLogs => false, // true ?
            TileType::Hedge => false,
            TileType::MiddleRock => false, // true ?
        }
    }
}

#[derive(Debug)]
pub enum TerrainTileError {
    UnknownId(String),
}

impl Display for TerrainTileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TerrainTileError::UnknownId(id) => f.write_str(&format!("Unknown id : {}", id)),
        }
    }
}
#[derive(Clone)]
pub struct TerrainTile {
    pub type_: TileType,
    pub tile_width: u32,
    pub tile_height: u32,
    pub relative_tile_width: f32,
    pub relative_tile_height: f32,
    pub x: u32,
    pub y: u32,
    pub tile_x: u32,
    pub tile_y: u32,
}

impl TerrainTile {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        type_: TileType,
        tile_width: u32,
        tile_height: u32,
        relative_tile_width: f32,
        relative_tile_height: f32,
        x: u32,
        y: u32,
        tile_x: u32,
        tile_y: u32,
    ) -> Self {
        Self {
            type_,
            tile_width,
            tile_height,
            relative_tile_width,
            relative_tile_height,
            x,
            y,
            tile_x,
            tile_y,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn from_str_id(
        id: &str,
        tile_width: u32,
        tile_height: u32,
        relative_tile_width: f32,
        relative_tile_height: f32,
        x: u32,
        y: u32,
        tile_x: u32,
        tile_y: u32,
    ) -> Result<Self, TerrainTileError> {
        Ok(Self::new(
            TileType::from_str(id)?,
            tile_width,
            tile_height,
            relative_tile_width,
            relative_tile_height,
            x,
            y,
            tile_x,
            tile_y,
        ))
    }

    pub fn type_(&self) -> &TileType {
        &self.type_
    }
}
