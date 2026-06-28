use crate::{
    behavior::{feeling::Feeling, gesture::Gesture, Behavior, Body},
    deployment::SoldierDeployment,
    game::{
        weapon::{Magazine, Shot, Weapon},
        Side,
    },
    graphics::{soldier::SoldierAnimationType, weapon::WeaponAnimationType},
    order::Order,
    types::*,
};
use oc_core::game::soldier::SoldierType;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct Soldier {
    uuid: SoldierIndex,
    type_: SoldierType,
    side: Side,
    world_point: WorldPoint,
    squad_uuid: SquadUuid,
    order: Order,
    behavior: Behavior,
    gesture: Gesture,
    looking_direction: Angle,
    alive: bool,
    unconscious: bool,
    under_fire: Feeling,
    main_weapon: Option<Weapon>,
    magazines: Vec<Magazine>,
    last_shoot_frame_i: u64,
    last_shot_frame_i: u64,
    support_promise_end_frame_i: u64,
    grenades: u8,
    last_grenade_frame_i: u64,
    #[serde(default)]
    last_hide_frame_i: u64,
}

impl Soldier {
    pub fn new(
        uuid: SoldierIndex,
        type_: SoldierType,
        world_point: WorldPoint,
        squad_uuid: SquadUuid,
        side: Side,
        main_weapon: Option<Weapon>,
        magazines: Vec<Magazine>,
        grenades: u8,
    ) -> Self {
        Self {
            uuid,
            type_,
            side,
            world_point,
            squad_uuid,
            order: Order::Idle,
            behavior: Behavior::Idle(Body::StandUp),
            gesture: Gesture::Idle,
            looking_direction: Angle(0.0),
            alive: true,
            unconscious: false,
            under_fire: Feeling::UnderFire(0),
            main_weapon,
            magazines,
            last_shot_frame_i: 0,
            last_shoot_frame_i: 0,
            support_promise_end_frame_i: 0,
            grenades,
            last_grenade_frame_i: 0,
            last_hide_frame_i: 0,
        }
    }

    pub fn from_soldier(soldier: &Soldier) -> Self {
        let mut new_soldier = Self::new(
            soldier.uuid(),
            *soldier.type_(),
            soldier.world_point(),
            soldier.squad_uuid(),
            *soldier.side(),
            soldier.main_weapon().clone(),
            soldier.magazines().clone(),
            soldier.grenades(),
        );
        new_soldier.last_grenade_frame_i = soldier.last_grenade_frame_i();
        new_soldier.last_hide_frame_i = soldier.last_hide_frame_i();
        new_soldier
    }

    pub fn uuid(&self) -> SoldierIndex {
        self.uuid
    }

    pub fn side(&self) -> &Side {
        &self.side
    }

    pub fn world_point(&self) -> WorldPoint {
        self.world_point
    }

    pub fn set_world_point(&mut self, point: WorldPoint) {
        self.world_point = point
    }

    pub fn squad_uuid(&self) -> SquadUuid {
        self.squad_uuid
    }

    pub fn set_squad_uuid(&mut self, squad_uuid: SquadUuid) {
        self.squad_uuid = squad_uuid;
    }

    pub fn behavior(&self) -> &Behavior {
        &self.behavior
    }

    pub fn behavior_mut(&mut self) -> &mut Behavior {
        &mut self.behavior
    }

    pub fn gesture(&self) -> &Gesture {
        &self.gesture
    }

    pub fn set_gesture(&mut self, gesture: Gesture) {
        self.gesture = gesture
    }

    pub fn order(&self) -> &Order {
        &self.order
    }

    pub fn order_mut(&mut self) -> &mut Order {
        &mut self.order
    }

    pub fn set_behavior(&mut self, behavior: Behavior) {
        self.behavior = behavior
    }

    pub fn set_order(&mut self, order: Order) {
        self.order = order
    }

    pub fn get_looking_direction(&self) -> Angle {
        self.looking_direction
    }

    pub fn set_looking_direction(&mut self, angle: Angle) {
        self.looking_direction = angle
    }

    pub fn main_weapon(&self) -> &Option<Weapon> {
        &self.main_weapon
    }

    pub fn magazines(&self) -> &Vec<Magazine> {
        &self.magazines
    }

    pub fn alive_mut(&mut self) -> &mut bool {
        &mut self.alive
    }

    pub fn unconscious_mut(&mut self) -> &mut bool {
        &mut self.unconscious
    }

    pub fn set_alive(&mut self, value: bool) {
        self.alive = value
    }

    pub fn set_unconscious(&mut self, value: bool) {
        self.unconscious = value
    }

    pub fn can_be_animated(&self) -> bool {
        self.alive && !self.unconscious && !matches!(self.behavior, Behavior::OffMapTransit(_))
    }

    pub fn can_be_leader(&self) -> bool {
        self.alive && !self.unconscious && !matches!(self.behavior, Behavior::OffMapTransit(_))
    }

    pub fn can_be_count_for_morale(&self) -> bool {
        // [버그 수정: 후퇴 중 사기 급락으로 인한 게임 종료 방지]
        // 일시적으로 맵 밖으로 이탈(OffMapTransit)한 병사라도 살아있으므로 사기 계산에 포함시켜야 게임이 돌연 종료되지 않습니다.
        self.alive && !self.unconscious
    }

    pub fn can_produce_sound(&self) -> bool {
        self.alive && !self.unconscious && !matches!(self.behavior, Behavior::OffMapTransit(_))
    }

    pub fn can_feel_explosion(&self) -> bool {
        self.alive && !matches!(self.behavior, Behavior::OffMapTransit(_))
    }

    pub fn can_feel_bullet_fire(&self) -> bool {
        self.alive && !matches!(self.behavior, Behavior::OffMapTransit(_))
    }

    pub fn can_see_interior(&self) -> bool {
        self.alive && !self.unconscious && !matches!(self.behavior, Behavior::OffMapTransit(_))
    }

    pub fn can_seek(&self) -> bool {
        self.alive && !self.unconscious && !matches!(self.behavior, Behavior::OffMapTransit(_))
    }

    pub fn can_be_designed_as_target(&self) -> bool {
        self.alive && !self.unconscious && !matches!(self.behavior, Behavior::OffMapTransit(_))
    }

    pub fn can_take_flag(&self) -> bool {
        self.can_be_animated()
    }

    pub fn under_fire(&self) -> &Feeling {
        &self.under_fire
    }

    pub fn under_fire_mut(&mut self) -> &mut Feeling {
        &mut self.under_fire
    }

    pub fn increase_under_fire(&mut self, value: u32) {
        self.under_fire.increase(value)
    }

    pub fn decrease_under_fire(&mut self) {
        self.under_fire.decrease()
    }

    pub fn set_last_shoot_frame_i(&mut self, value: u64) {
        self.last_shoot_frame_i = value
    }

    pub fn last_shoot_frame_i(&self) -> &u64 {
        &self.last_shoot_frame_i
    }

    pub fn set_last_shot_frame_i(&mut self, value: u64) {
        self.last_shot_frame_i = value
    }

    pub fn last_shot_frame_i(&self) -> &u64 {
        &self.last_shot_frame_i
    }

    pub fn set_support_promise_end_frame_i(&mut self, value: u64) {
        self.support_promise_end_frame_i = value
    }

    pub fn support_promise_end_frame_i(&self) -> &u64 {
        &self.support_promise_end_frame_i
    }

    pub fn grenades(&self) -> u8 {
        self.grenades
    }

    pub fn set_grenades(&mut self, val: u8) {
        self.grenades = val;
    }

    pub fn last_grenade_frame_i(&self) -> u64 {
        self.last_grenade_frame_i
    }

    pub fn set_last_grenade_frame_i(&mut self, val: u64) {
        self.last_grenade_frame_i = val;
    }

    pub fn last_hide_frame_i(&self) -> u64 {
        self.last_hide_frame_i
    }

    pub fn set_last_hide_frame_i(&mut self, val: u64) {
        self.last_hide_frame_i = val;
    }

    pub fn replenish_ammunition(&mut self) {
        if let Some(weapon) = &self.main_weapon {
            let mut new_mags = vec![];
            let default_mag = match weapon {
                Weapon::MosinNagantM1924(_, _) => Magazine::MosinNagant(5),
                Weapon::MauserG41(_, _) => Magazine::Mauser(5),
                Weapon::BrenMark2(_) => Magazine::BrenCurved30(30),
                Weapon::Mg34(_) => Magazine::Patronengurtx792x57s250(250),
            };
            for _ in 0..weapon.ok_count_magazines() {
                new_mags.push(default_mag.clone());
            }
            self.magazines = new_mags;
            self.grenades = 3;
        }
    }

    pub fn weapon(&self, class: &WeaponClass) -> &Option<Weapon> {
        match class {
            WeaponClass::Main => &self.main_weapon,
        }
    }

    pub fn weapon_mut(&mut self, class: &WeaponClass) -> &mut Option<Weapon> {
        match class {
            WeaponClass::Main => &mut self.main_weapon,
        }
    }

    pub fn reload_weapon(&mut self, class: &WeaponClass) {
        let mut magazines = self.magazines.clone();
        if let Some(weapon) = self.weapon_mut(class) {
            weapon.reload();
            if weapon.magazine().is_none() {
                while let Some(magazine) = magazines.pop() {
                    if weapon.accepted_magazine(&magazine) {
                        weapon.set_magazine(magazine);
                        break;
                    } else {
                        magazines.push(magazine)
                    }
                }
            }
        } else {
            eprintln!("Tried to reload weapon class {:?} but no weapon", class)
        }
        self.magazines = magazines;
    }

    pub fn weapon_shot(&mut self, class: &WeaponClass, shot: &Shot) {
        if let Some(weapon) = self.weapon_mut(class) {
            weapon.shot(shot);
        }
    }

    pub fn alive(&self) -> bool {
        self.alive
    }

    pub fn type_(&self) -> &SoldierType {
        &self.type_
    }

    pub fn unconscious(&self) -> bool {
        self.unconscious
    }

    pub fn target(&self) -> Option<&SoldierIndex> {
        match self.behavior() {
            Behavior::EngageSoldier(soldier_index) => Some(soldier_index),
            _ => None,
        }
    }

    pub fn animation_type(&self) -> (SoldierAnimationType, WeaponAnimationType) {
        let animation_type = match self.behavior() {
            Behavior::Idle(Body::StandUp) => SoldierAnimationType::Idle,
            Behavior::Idle(Body::Crouched) => SoldierAnimationType::Idle,
            Behavior::Idle(Body::Lying) => SoldierAnimationType::Crawling,
            Behavior::MoveTo(_) => SoldierAnimationType::Walking,
            Behavior::MoveFastTo(_) => SoldierAnimationType::Walking,
            Behavior::SneakTo(_) => SoldierAnimationType::Crawling,
            Behavior::Defend(_) => SoldierAnimationType::LyingDown,
            Behavior::Hide(_) | Behavior::ScatterToCover(_) | Behavior::GatherToCover(_) => SoldierAnimationType::LyingDown,
            Behavior::DriveTo(_) => SoldierAnimationType::Idle,
            Behavior::RotateTo(_) => SoldierAnimationType::Idle,
            // TODO : Different animation according to death type
            Behavior::Dead => SoldierAnimationType::DeadWithSideBlood,
            Behavior::Unconscious => SoldierAnimationType::LyingDown,
            Behavior::SuppressFire(_) => SoldierAnimationType::LyingDown,
            Behavior::EngageSoldier(_) => SoldierAnimationType::LyingDown,
            Behavior::OffMapTransit(_) => SoldierAnimationType::Idle,
        };

        let weapon_animation_type = WeaponAnimationType::from(&animation_type);
        (animation_type, weapon_animation_type)
    }

    pub fn body(&self) -> Body {
        match self.behavior {
            Behavior::MoveTo(_) => Body::StandUp,
            Behavior::MoveFastTo(_) => Body::StandUp,
            Behavior::SneakTo(_) => Body::Lying,
            Behavior::DriveTo(_) => Body::Crouched,
            Behavior::RotateTo(_) => Body::Crouched,
            Behavior::Idle(body) => body,
            Behavior::Defend(_) => Body::Lying,
            Behavior::Hide(_) | Behavior::ScatterToCover(_) | Behavior::GatherToCover(_) => Body::Lying,
            Behavior::Dead => Body::Lying,
            Behavior::Unconscious => Body::Lying,
            Behavior::SuppressFire(_) => Body::Lying,
            Behavior::EngageSoldier(_) => Body::Lying,
            Behavior::OffMapTransit(_) => Body::StandUp,
        }
    }
}

impl From<&SoldierDeployment> for Soldier {
    fn from(deployment: &SoldierDeployment) -> Self {
        let mut soldier = Self::new(
            deployment.uuid(),
            *deployment.type_(),
            deployment.world_point(),
            deployment.squad_uuid(),
            deployment.side(),
            deployment.main_weapon().cloned(),
            deployment.magazines().to_vec(),
            deployment.grenades(),
        );
        soldier.order = deployment.order().clone();
        soldier.behavior = deployment.behavior().clone();
        soldier
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WeaponClass {
    Main,
}
