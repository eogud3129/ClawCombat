use std::collections::HashMap;

use oc_core::{
    game::{soldier::SoldierType, squad::SquadType},
    graphics::ammunition::AmmunitionReserveStatus,
    health::Health,
    morale::Morale,
};

use crate::{
    behavior::{feeling::UNDER_FIRE_MAX, gesture::Gesture, Behavior},
    entity::soldier::Soldier,
    state::battle::BattleState,
    types::{SoldierIndex, SquadComposition, SquadUuid, WorldPoint},
    utils::apply_angle_on_point,
};

use super::{
    health::SoldierHealthBuilder,
    weapon::{Magazine, Weapon},
    Side,
};

pub enum Formation {
    Line,
}

pub fn squad_positions(
    squad: &SquadComposition,
    formation: Formation,
    leader: &Soldier,
    point: Option<WorldPoint>,
) -> HashMap<SoldierIndex, WorldPoint> {
    let mut positions = HashMap::new();
    let ref_point = point.unwrap_or(leader.world_point());
    let ref_angle = leader.get_looking_direction();

    match formation {
        Formation::Line => {
            let mut x_offset: f32 = 0.0;
            let mut y_offset: f32 = 0.0;
            let mut counter: u8 = 0;

            for (i, soldier_index) in squad.members().iter().enumerate() {
                // Don't return position for leader
                if *soldier_index == squad.leader() {
                    continue;
                }

                if counter % 2 == 0 {
                    // [버그 수정: Bounding Overwatch(교대 전진) 정상화]
                    // 산개 간격이 25m로 넓으면 부하들이 항상 분대장과 5m 이상 떨어져 있게 되어, 
                    // 분대장은 영원히 멈추고 부하 3명만 계속 움직이는(3명만 움직임) 버그가 발생합니다.
                    // 대형 간격을 2m로 좁혀 부하들이 분대장 근처(5m 이내)로 완벽히 집결할 수 있도록 수정합니다.
                    x_offset += 2.0; 
                    y_offset += 2.0; 
                }
                counter += 1;

                let (x_offset_, y_offset_) = if i % 2 == 0 {
                    (x_offset, y_offset)
                } else {
                    // 양쪽 날개로 산개하도록 -y가 아닌 +y로 통일하여 완벽한 V자(Wedge) 대형 구축
                    (-x_offset, y_offset)
                };

                let member_scene_point =
                    WorldPoint::new(ref_point.x + x_offset_, ref_point.y + y_offset_);
                let member_scene_point =
                    apply_angle_on_point(&member_scene_point, &ref_point, &ref_angle);
                positions.insert(*soldier_index, member_scene_point);
            }
        }
    }

    positions
}

pub struct SquadStatusesResume {
    squads: Vec<SquadStatusResume>,
}

impl SquadStatusesResume {
    pub fn from_battle_state(side: &Side, battle_state: &BattleState) -> Self {
        Self {
            squads: battle_state
                .squads()
                .iter()
                .filter(|(_, squad)| battle_state.soldier(squad.leader()).side() == side)
                .map(|(squad_id, _)| SquadStatusResume::from_squad(battle_state, squad_id))
                .collect(),
        }
    }

    pub fn squads(&self) -> &[SquadStatusResume] {
        self.squads.as_ref()
    }
}

#[derive(Clone, Debug)]
pub struct SquadStatusResume {
    squad_id: SquadUuid,
    squad_type: SquadType,
    health: SquadHealth,
    members: Vec<SquadMemberStatus>,
}

impl SquadStatusResume {
    pub fn from_squad(battle_state: &BattleState, squad_id: &SquadUuid) -> Self {
        let squad = battle_state.squad(*squad_id);
        Self {
            squad_id: *squad_id,
            squad_type: *battle_state.squad_type(squad_id),
            health: SquadHealth::from_squad(battle_state, squad),
            members: squad
                .members()
                .iter()
                .map(|soldier_index| {
                    SquadMemberStatus::from_soldier(
                        battle_state,
                        squad,
                        battle_state.soldier(*soldier_index),
                    )
                })
                .collect(),
        }
    }

    pub fn health(&self) -> &SquadHealth {
        &self.health
    }

    pub fn members(&self) -> &[SquadMemberStatus] {
        self.members.as_ref()
    }

    pub fn squad_id(&self) -> &SquadUuid {
        &self.squad_id
    }

    pub fn squad_type(&self) -> &SquadType {
        &self.squad_type
    }
}

#[derive(Clone, Debug)]
pub struct SquadHealth(pub f32);

impl SquadHealth {
    pub fn from_squad(battle_state: &BattleState, squad: &SquadComposition) -> Self {
        let total = squad.members().len();
        let readies = squad
            .members()
            .iter()
            .filter(|soldier_index| {
                battle_state
                    .soldier(**soldier_index)
                    .can_be_count_for_morale()
            })
            .collect::<Vec<&SoldierIndex>>()
            .len();
        Self(readies as f32 / total as f32)
    }

    pub fn into_morale(&self) -> Morale {
        Morale(self.0)
    }
}

#[derive(Clone, Debug)]
pub struct SquadMemberStatus {
    soldier_index: SoldierIndex,
    type_: SoldierType,
    health: Health,
    main_weapon: Option<Weapon>,
    magazines: Vec<Magazine>,
    ammunition_reserve: AmmunitionReserveStatus,
    under_fire_coefficient: f32,
    current: CurrentAction,
    leader: bool,
}

// FIXME : this func is here because AmmunitionReserveStatus, Soldier, etc will have to move
// into oc_core ...
fn ammunition_reserve_status(soldier: &Soldier) -> AmmunitionReserveStatus {
    if let Some(weapon) = soldier.main_weapon() {
        let ok_magazines_len = soldier
            .magazines()
            .iter()
            .filter(|m| weapon.accepted_magazine(m))
            .collect::<Vec<&Magazine>>()
            .len();
        if ok_magazines_len == 0 {
            return AmmunitionReserveStatus::Empty;
        }

        if ok_magazines_len < weapon.ok_count_magazines() {
            return AmmunitionReserveStatus::Low;
        }
    }

    AmmunitionReserveStatus::Ok
}

impl SquadMemberStatus {
    pub fn from_soldier(
        battle_state: &BattleState,
        squad: &SquadComposition,
        soldier: &Soldier,
    ) -> Self {
        Self {
            soldier_index: soldier.uuid(),
            type_: *soldier.type_(),
            health: SoldierHealthBuilder::new(soldier).build(),
            main_weapon: soldier.main_weapon().clone(),
            magazines: soldier.magazines().clone(),
            ammunition_reserve: ammunition_reserve_status(soldier),
            under_fire_coefficient: (*soldier.under_fire().value() as f32 / UNDER_FIRE_MAX as f32),
            current: CurrentAction::from_soldier(battle_state, squad, soldier),
            leader: battle_state.squad(soldier.squad_uuid()).leader() == soldier.uuid(),
        }
    }

    pub fn main_weapon(&self) -> Option<&Weapon> {
        self.main_weapon.as_ref()
    }

    pub fn health(&self) -> &Health {
        &self.health
    }

    pub fn magazines(&self) -> &[Magazine] {
        self.magazines.as_ref()
    }

    pub fn under_fire_coefficient(&self) -> f32 {
        self.under_fire_coefficient
    }

    pub fn current(&self) -> &CurrentAction {
        &self.current
    }

    pub fn ammunition_reserve(&self) -> &AmmunitionReserveStatus {
        &self.ammunition_reserve
    }

    pub fn leader(&self) -> bool {
        self.leader
    }

    pub fn soldier_index(&self) -> SoldierIndex {
        self.soldier_index
    }

    pub fn type_(&self) -> SoldierType {
        self.type_
    }
}

#[derive(Clone, Debug)]
pub enum CurrentAction {
    Idle,
    Walking,
    Running,
    Crawling,
    // Targeting,
    TargetFiring,
    SuppressFiring,
    Aiming,
    Reloading,
    Defending,
    Hiding,
    Driving,
    Rotating,
    Throwing,
    // ...
}

impl CurrentAction {
    pub fn from_soldier(
        _battle_state: &BattleState,
        _squad: &SquadComposition,
        soldier: &Soldier,
    ) -> Self {
        if matches!(soldier.gesture(), Gesture::Throwing(_, _)) {
            return Self::Throwing;
        }

        match soldier.behavior() {
            Behavior::MoveTo(_) => Self::Walking,
            Behavior::MoveFastTo(_) => Self::Running,
            Behavior::SneakTo(_) => Self::Crawling,
            Behavior::DriveTo(_) => Self::Driving,
            Behavior::RotateTo(_) => Self::Rotating,
            Behavior::Defend(_) => Self::Defending,
            Behavior::Hide(_) | Behavior::ScatterToCover(_) | Behavior::GatherToCover(_) => Self::Hiding,
            Behavior::SuppressFire(_) => match soldier.gesture() {
                Gesture::Idle => Self::Idle,
                Gesture::Reloading(_, _) => Self::Reloading,
                Gesture::Aiming(_, _) => Self::Aiming,
                Gesture::Firing(_, _) => Self::SuppressFiring,
                Gesture::Throwing(_, _) => Self::Throwing,
            },
            Behavior::EngageSoldier(_) => match soldier.gesture() {
                Gesture::Idle => Self::Idle,
                Gesture::Reloading(_, _) => Self::Reloading,
                Gesture::Aiming(_, _) => Self::Aiming,
                Gesture::Firing(_, _) => Self::TargetFiring,
                Gesture::Throwing(_, _) => Self::Throwing,
            },
            Behavior::Idle(_) | Behavior::Dead | Behavior::Unconscious | Behavior::OffMapTransit(_) => Self::Idle,
        }
    }

    pub fn display(&self) -> &str {
        match self {
            CurrentAction::Idle => "",
            CurrentAction::Walking => "move",
            CurrentAction::Running => "move fast",
            CurrentAction::Crawling => "crawling",
            CurrentAction::TargetFiring => "firing target",
            CurrentAction::SuppressFiring => "suppress firing",
            CurrentAction::Aiming => "aiming",
            CurrentAction::Reloading => "reloading",
            CurrentAction::Defending => "defending",
            CurrentAction::Hiding => "hiding",
            CurrentAction::Driving => "driving",
            CurrentAction::Rotating => "rotating",
            CurrentAction::Throwing => "throwing grenade",
        }
    }
}
