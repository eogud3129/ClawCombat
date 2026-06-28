use battle_core::{
    behavior::gesture::{Gesture, GestureContext},
    entity::soldier::{Soldier, WeaponClass},
    game::weapon::Weapon,
    order::Order,
    physics::{utils::distance_between_points, visibility::Visibility},
    types::WorldPoint,
};
use glam::Vec2;
use rand::Rng;

use crate::runner::Runner;

impl Runner {
    pub fn soldier_able_to_fire_on_point<'a>(
        &'a self,
        soldier: &'a Soldier,
        point: &WorldPoint,
    ) -> Option<(WeaponClass, &'a Weapon, Visibility)> {
        let visibility = self.battle_state.point_is_visible_by_soldier(
            &self.config,
            soldier,
            point,
            // Shoot a hidden point is possible (like fire through a wall)
            self.config.visibility_by_last_frame_shoot_distance,
        );

        if visibility.blocked {
            return None;
        }

        // [지형 불투명도 시야 패널티 미적용 버그 수정]
        // 수풀이나 나무 지형은 고체 벽(blocked)이 아니기 때문에, 불투명도 누적으로 인해 실제 시야 밖(!visible)으로
        // 판정되더라도 사격을 감행하는 심각한 연산 누수가 있었습니다. 가시성 유효 플래그를 엄격하게 대조 검사합니다.
        if !visibility.visible {
            return None;
        }

        if let Some((weapon_class, weapon)) = self.soldier_weapon_for_point(soldier, point) {
            // [수류탄 투척거리 및 아군 안전거리 필터 적용]
            // 수류탄(Grenade) 계열 무기일 경우 투척 거리를 제한하고 오폭을 방지합니다.
            if weapon.name().contains("Grenade") {
                // 1. 투척 사거리 제한 (15m 초과 시 투척 불가)
                if visibility.distance.meters() > 15 {
                    return None;
                }
                
                // 2. 아군 오폭 방지 (타겟 반경 6m 이내에 살아있는 아군이 있으면 투척 취소)
                let mut ally_in_danger = false;
                for ally in self.battle_state.soldiers() {
                    if ally.side() == soldier.side() && ally.alive() {
                        let distance_to_blast = battle_core::physics::utils::distance_between_points(&ally.world_point(), point);
                        if distance_to_blast.meters() <= 6 {
                            ally_in_danger = true;
                            break;
                        }
                    }
                }
                
                if ally_in_danger {
                    return None; // 아군이 위험 반경에 있으므로 사격(투척) 취소
                }
            }

            // 참고: 수류탄의 '투척 횟수' 차감은 아래 weapon.can_fire() 및 이후 엔진의 weapon.shot() 단계에서
            // 탄창(Magazine) 소모 로직을 통해 소총탄과 동일하게 자동으로 차감되도록 시스템이 구성되어 있습니다.
            if weapon.can_fire() || weapon.can_reload() {
                return Some((weapon_class, weapon, visibility));
            }

            if self.soldier_can_reload_with(soldier, weapon).is_some() {
                return Some((weapon_class, weapon, visibility));
            }
        }

        None
    }

    pub fn engage_point_gesture(
        &self,
        soldier: &Soldier,
        engagement: (WeaponClass, &Weapon, Visibility),
    ) -> (GestureContext, Gesture) {
        let frame_i = self.battle_state.frame_i();
        let current = soldier.gesture();
        let (weapon_class, weapon, visibility) = engagement;

        // [경직 고착화(Permastun) 버그 수정]
        // 기존 45프레임 하드락(Hard Lock)은 기관총 제압 사격 시 병사를 영구적으로 마비시켜 수류탄조차 
        // 못 던지게 만드는 치명적 결함이 있었습니다. 이를 완전히 제거하고 하단의 Peek & Shoot
        // (스트레스 비례 Hide 프레임 연장) 시스템으로 제압 효과를 자연스럽게 이관합니다.

        // [스트레스에 따른 숨는 자세 (Peek & Shoot) 강제 적용]
        // 방어/은폐 명령을 수행 중이거나, 스트레스가 Max나 Danger일 경우 엄폐 시간 비율을 조절하여 생존 사격 모드로 전환합니다.
        let is_cautious_mode = matches!(soldier.order(), Order::Defend(_) | Order::Hide(_));
        let is_stressed = soldier.under_fire().is_danger() || soldier.under_fire().is_max();

        if (is_cautious_mode || is_stressed) && current == &Gesture::Idle {
            // [사격 지연 프레임 동기화] soldier.rs에서 계산되는 stress_delay를 포함하여 실제 엔진 소모 프레임을 계산
            let stress_delay_aim = if soldier.under_fire().is_max() { 40 } else if soldier.under_fire().is_danger() { 20 } else { 0 };
            let stress_delay_fire = if soldier.under_fire().is_max() { 30 } else if soldier.under_fire().is_danger() { 15 } else { 0 };
            
            // 실제 조준 및 사격에 소요되는 전체 활성 프레임 합산
            let active_frames = weapon.aiming_frames() + stress_delay_aim + weapon.firing_frames() + stress_delay_fire;
            
            let hide_frames = if soldier.under_fire().is_max() {
                active_frames * 4 // Max 상태: 20% 사격 / 80% 엄폐
            } else if soldier.under_fire().is_danger() || is_cautious_mode {
                active_frames // Danger 상태: 50% 사격 / 50% 엄폐
            } else {
                0
            };

            // 마지막 사격 개시 프레임(last_shoot_frame_i) 기준 다음 사격 가능 타이밍 계산
            let next_allowed_shoot_frame = soldier.last_shoot_frame_i() + active_frames + hide_frames;

            // 아직 엄폐해야 하는 쿨타임(대기 시간)이라면 사격을 개시하지 않고 Idle(엄폐) 상태 강제 유지
            if *frame_i < next_allowed_shoot_frame {
                return (
                    GestureContext::Idle,
                    Gesture::Idle, 
                );
            }
        }

        let gesture = match current {
            Gesture::Idle => {
                if weapon.can_fire() {
                    Gesture::Aiming(
                        self.soldier_aiming_end(soldier, weapon),
                        weapon_class.clone(),
                    )
                } else {
                    Gesture::Reloading(
                        self.soldier_reloading_end(soldier, weapon),
                        weapon_class.clone(),
                    )
                }
            }
            Gesture::Reloading(_, _) => {
                //
                current.next(
                    *frame_i,
                    Gesture::Aiming(
                        self.soldier_aiming_end(soldier, weapon),
                        weapon_class.clone(),
                    ),
                )
            }
            Gesture::Aiming(_, _) => {
                //
                let end = self.soldier_firing_end(soldier, weapon);
                current.next(*frame_i, Gesture::Firing(end, weapon_class.clone()))
            }
            Gesture::Firing(_, _) => {
                //
                current.next(*frame_i, Gesture::Idle)
            }
            Gesture::Throwing(_, _) => {
                // 수류탄 투척 중 사격 명령이 들어오면 기존 투척 상태를 유지하여 모션이 끊기지 않도록 방어합니다.
                current.clone()
            }
        };

        let final_point = self.soldier_fire_point(soldier, &weapon_class, &visibility.altered_to);
        (
            GestureContext::Firing(final_point, None, visibility),
            gesture,
        )
    }

    // FIXME : use realistic range error (angle from target)
    pub fn soldier_fire_point(
        &self,
        soldier: &Soldier,
        _weapon_class: &WeaponClass,
        target_point: &WorldPoint,
    ) -> WorldPoint {
        let mut rng = rand::thread_rng();
        // TODO : change precision according to weapon, stress, distance, etc
        let mut factor_by_meter = self.config.inaccurate_fire_factor_by_meter;

        // [스트레스 임계치 초과 시 조준 정확도(명중률) 대폭 하락]
        // 사격자의 스트레스가 높을수록 조준이 흔들려 탄착군 오차 범위를 대폭 증폭시킴
        if soldier.under_fire().is_max() {
            factor_by_meter *= 2.5; // 공포에 질린 상태: 오차 2.5배 증가 (난사 효과)
        } else if soldier.under_fire().is_danger() {
            factor_by_meter *= 1.5; // 위험 상태: 오차 1.5배 증가
        }

        let distance = distance_between_points(&soldier.world_point(), target_point);

        // [Part 1: 지향 사격(Move & Shoot) 명중률 밸런싱]
        // 기동 중(MoveTo, MoveFastTo)일 때는 거리에 비례하여 오차를 동적으로 조절합니다.
        if matches!(soldier.behavior(), battle_core::behavior::Behavior::MoveTo(_) | battle_core::behavior::Behavior::MoveFastTo(_)) {
            if distance.meters() <= 15 {
                factor_by_meter *= 1.5; // 근접(CQB)에서는 지향 사격 페널티 완화
            } else {
                factor_by_meter *= 3.0; // 원거리에서는 기존처럼 3배 오차 적용 (지향 사격의 한계)
            }
        }
        let range = distance.meters() as f32 * factor_by_meter;

        // [Part 3: 초근접 영거리 사격 100% 명중 보정]
        // 거리가 1m 미만이라 range가 0.0이 될 경우, 불필요한 에러 로그 폭주(I/O 프리징)를 막고 
        // 오차 연산을 바이패스하여 타겟의 정중앙(100% 명중) 좌표를 완벽하게 반환합니다.
        if range == 0. {
            return *target_point;
        }

        let x_change = rng.gen_range(-range..range);
        let y_change = rng.gen_range(-range..range);

        target_point.apply(Vec2::new(x_change, y_change))
    }
}
