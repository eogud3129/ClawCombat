use battle_core::{
    behavior::{
        gesture::{Gesture, GestureContext},
        Behavior,
    },
    entity::soldier::{Soldier, WeaponClass},
    game::{
        weapon::Weapon,
        Side,
    },
    physics::{
        event::{bullet::BulletFire, cannon_blast::CannonBlast},
        utils::distance_between_points,
        visibility::Visibility,
    },
    state::{
        battle::message::{BattleStateMessage, SoldierMessage},
        client::ClientStateMessage,
    },
    types::{Distance, Precision, SoldierIndex, WorldPoint},
};
use glam::Vec2;
use rand::Rng;

use super::{fight::choose::ChooseMethod, message::RunnerMessage, Runner};

mod engage;
mod fire;
mod idle;
mod soldier;
mod suppress;
mod weapon;

pub struct FallbackBehavior(pub Behavior);

pub enum GestureResult {
    Handled(GestureContext, Gesture),
    Cant(Option<FallbackBehavior>),
}

impl Runner {
pub fn soldier_gesture(&self, soldier: &Soldier) -> Vec<RunnerMessage> {
    puffin::profile_scope!("soldier_gesture");
    let mut messages = vec![];

    // 플레이어가 직접 조종하는 유닛은 AI가 제스처를 변경하지 않음
    if soldier.player_controlled() {
        return messages;
    }

    // [수류탄 투척 제스처 락]
    if let Gesture::Throwing(_, _) = soldier.gesture() {
        return messages;
    }

        let new_gesture = match soldier.behavior() {
            Behavior::Idle(_) => {
                //
                self.idle_gesture(soldier)
            }
            Behavior::SuppressFire(point) => {
                //
                self.suppress_fire_gesture(soldier, point)
            }
            Behavior::EngageSoldier(soldier_index) => {
                //
                self.engage_soldier_gesture(soldier, soldier_index)
            }
            Behavior::MoveTo(_) | Behavior::MoveFastTo(_) => {
                // [Part 3: 사격과 기동의 분리 (Move & Shoot)]
                // 기동 중이더라도 적이 전방 시야(50m 이내)에 포착되면 이동을 멈추지 않고 걸어가며 지향 사격을 뿌립니다.
                let mut result = GestureResult::Handled(GestureContext::Idle, Gesture::Idle);
                if let Some(opponent) = self.soldier_find_opponent_to_target(soldier, None, &ChooseMethod::RandomFromNearest) {
                    let point = opponent.world_point();
                    let dist = distance_between_points(&soldier.world_point(), &point).meters();
                    if dist <= 50 {
                        if let Some(engagement) = self.soldier_able_to_fire_on_point(soldier, &point) {
                            let (context, gesture) = self.engage_point_gesture(soldier, engagement);
                            result = GestureResult::Handled(context, gesture);
                        }
                    }
                }
                result
            }
            _ => GestureResult::Handled(GestureContext::Idle, Gesture::Idle),
        };

        match new_gesture {
            GestureResult::Handled(context, gesture) => {
                if &gesture != soldier.gesture() {
                    return [
                        self.new_gesture_messages(soldier, &context, &gesture),
                        vec![RunnerMessage::BattleState(BattleStateMessage::Soldier(
                            soldier.uuid(),
                            SoldierMessage::SetGesture(gesture),
                        ))],
                    ]
                    .concat();
                }
            }
            GestureResult::Cant(fallback) => {
                if let Some(fallback) = fallback {
                    messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                        soldier.uuid(),
                        SoldierMessage::SetBehavior(fallback.0),
                    )));
                }
            }
        }

        messages
    }

    fn new_gesture_messages(
        &self,
        soldier: &Soldier,
        context: &GestureContext,
        gesture: &Gesture,
    ) -> Vec<RunnerMessage> {
        match (context, gesture) {
            (_, Gesture::Idle) => {}
            (_, Gesture::Reloading(_, class)) => {
                if let Some(weapon) = soldier.weapon(class) {
                    return self.reloading_gesture_messages(soldier, class, weapon);
                }
            }
            (_, Gesture::Aiming(_, _)) => {}
            (GestureContext::Firing(point, target, visibility), Gesture::Firing(_, class)) => {
                if let Some(weapon) = soldier.weapon(class) {
                    return self.firing_gesture_messages(
                        soldier, class, weapon, point, target, visibility,
                    );
                }
            }
            _ => {}
        }

        vec![]
    }

    pub fn reloading_gesture_messages(
        &self,
        soldier: &Soldier,
        class: &WeaponClass,
        weapon: &Weapon,
    ) -> Vec<RunnerMessage> {
        [
            vec![RunnerMessage::BattleState(BattleStateMessage::Soldier(
                soldier.uuid(),
                SoldierMessage::ReloadWeapon(class.clone()),
            ))],
            weapon
                .reload_sounds()
                .iter()
                .map(|sound| {
                    RunnerMessage::ClientsState(ClientStateMessage::PlayBattleSound(*sound))
                })
                .collect(),
        ]
        .concat()
    }

    pub fn firing_gesture_messages(
        &self,
        soldier: &Soldier,
        class: &WeaponClass,
        weapon: &Weapon,
        point: &WorldPoint,
        target: &Option<(SoldierIndex, Precision)>,
        visibility: &Visibility,
    ) -> Vec<RunnerMessage> {
        let mut rng = rand::thread_rng();
        // TODO: value in config
        let opponents_around = self.count_opponents_around(
            &soldier.side().opposite(),
            &visibility.to,
            Distance::from_meters(5),
        );
        let shot = weapon.shot_type(opponents_around);

        // FIXME BS NOW: generate multiple BulletFire & CannonBlast
        // FIXME BS NOW: warn about sound !

        let bullet_fires = (0..shot.count())
            .map(|i| {
                let point = if i > 0 {
                    let weapon_factor_multiplier = weapon.range_on_burst();
                    let factor_by_meter = self.config.inaccurate_fire_factor_by_meter;
                    let distance = visibility.distance;
                    let range =
                        distance.meters() as f32 * factor_by_meter * weapon_factor_multiplier;
                    if range > 0. {
                        let x_change = rng.gen_range(-range..range);
                        let y_change = rng.gen_range(-range..range);
                        point.apply(Vec2::new(x_change, y_change))
                    } else {
                        *point
                    }
                } else {
                    *point
                };
                let sound = if i == 0 {
                    Some(weapon.gun_fire_sound_type())
                } else {
                    None
                };
                RunnerMessage::BattleState(BattleStateMessage::PushBulletFire(BulletFire::new(
                    weapon.frame_offset_on_burst() * i as u64,
                    soldier.world_point(),
                    point,
                    target.clone(),
                    weapon.ammunition(),
                    sound,
                    shot,
                )))
            })
            .collect();

        let mut base_messages = vec![
            RunnerMessage::BattleState(BattleStateMessage::Soldier(
                soldier.uuid(),
                SoldierMessage::WeaponShot(class.clone(), shot),
            )),
            RunnerMessage::BattleState(BattleStateMessage::PushCannonBlast(CannonBlast::new(
                soldier.world_point(),
                soldier.get_looking_direction(),
                weapon.sprite_type(),
                soldier.animation_type().0,
            ))),
            RunnerMessage::BattleState(BattleStateMessage::Soldier(
                soldier.uuid(),
                SoldierMessage::SetLastShootFrameI(*self.battle_state.frame_i()),
            )),
        ];

        // [Part 1: 기동 간 제압(Suppression) 특화 보너스]
        // 이동 중 사격은 명중률이 떨어지는 대신 화망을 형성하여 적의 스트레스를 높이는 데 특화되어 있습니다.
        if matches!(soldier.behavior(), battle_core::behavior::Behavior::MoveTo(_) | battle_core::behavior::Behavior::MoveFastTo(_)) {
            if let Some((target_idx, _)) = target {
                base_messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                    *target_idx,
                    SoldierMessage::IncreaseUnderFire(30), // 제압 보너스 직접 부여
                )));
            }
        }

        [
            base_messages,
            bullet_fires,
        ]
        .concat()
    }

    fn count_opponents_around(&self, side: &Side, point: &WorldPoint, distance: Distance) -> usize {
        self.battle_state
            .soldiers()
            .iter()
            .filter(|s| s.side() == side)
            .filter(|s| s.can_be_designed_as_target())
            .filter(|s| {
                distance_between_points(&s.world_point(), point).millimeters()
                    <= distance.millimeters()
            })
            .collect::<Vec<&Soldier>>()
            .len()
    }
}
