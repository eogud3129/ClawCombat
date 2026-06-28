use battle_core::{
    behavior::feeling::Feeling,
    physics::{utils::distance_between_points, visibility::Visibility},
    state::battle::message::{BattleStateMessage, SoldierMessage},
    types::{Distance, SoldierIndex},
};

use crate::runner::{message::RunnerMessage, Runner};

impl Runner {
    // TODO : have a real algorithm here
    pub fn soldier_bullet_injured(&self, _soldier_index: SoldierIndex) -> Vec<RunnerMessage> {
        vec![]
    }

    // TODO : have a real algorithm here
    pub fn soldier_proximity_bullet(
        &self,
        soldier_index: SoldierIndex,
        distance: &Distance,
    ) -> Vec<RunnerMessage> {
        let soldier = self.battle_state.soldier(soldier_index);
        let mut friends_nearby = 0;
        
        // 주변 3m 이내에 살아있는 아군이 몇 명인지 계산 (군중 심리 및 공포 전염)
        for other in self.battle_state.soldiers() {
            if other.side() == soldier.side() && other.uuid() != soldier.uuid() && other.alive() {
                if distance_between_points(&soldier.world_point(), &other.world_point()).meters() < 3 {
                    friends_nearby += 1;
                }
            }
        }

        let base_stress = Feeling::proximity_bullet_increase_value(*distance);
        
        // 아군이 2명 이상 밀집해 있을 경우 스트레스 1.5배 패널티 적용
        let mut final_stress = if friends_nearby >= 2 {
            (base_stress as f32 * 1.5) as u32
        } else {
            base_stress
        };

        // [통합 위협 지수 기반 상대적 화력 우세 계산 (개별 병사 시야 반경 내)]
        // Fractional 가중치 적용을 위해 f32로 선언합니다.
        let mut ally_fire_count: f32 = 1.0; // 0으로 나누기 방지
        let mut enemy_fire_count: f32 = 1.0;

        // 1. 소총/기관총 사격 카운트
        for bullet in self.battle_state.bullet_fires() {
            if bullet.effective(*self.battle_state.frame_i()) {
                let dist_to_shooter = distance_between_points(&soldier.world_point(), bullet.from());
                let meters = dist_to_shooter.meters() as f32;
                if meters <= 50.0 {
                    // [시야 위협 차단 필터] 벽이나 장애물에 가려진 사격이면 공포 가중치를 절반(0.5)으로 줄입니다.
                    let visibility = Visibility::between_points(
                        &self.config,
                        &soldier.world_point(),
                        bullet.from(),
                        self.battle_state.map()
                    );
                    let visibility_multiplier = if visibility.blocked { 0.5 } else { 1.0 };

                    let mut found = false;
                    for shooter in self.battle_state.soldiers() {
                        if shooter.alive() && distance_between_points(&shooter.world_point(), bullet.from()).meters() < 2 {
                            if shooter.side() == soldier.side() { 
                                ally_fire_count += 1.0 * visibility_multiplier; 
                            } else { 
                                enemy_fire_count += 1.0 * visibility_multiplier; 
                            }
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        // 보병이 쏜 게 아니면 전차 기관총일 수 있으므로 탑승자를 통해 진영 식별
                        for vehicle in self.battle_state.vehicles() {
                            if distance_between_points(&vehicle.world_point(), bullet.from()).meters() < 5 {
                                if let Some(board) = self.battle_state.vehicle_board().get(vehicle.uuid()) {
                                    if let Some((_, occupant_idx)) = board.first() {
                                        let occupant = self.battle_state.soldier(*occupant_idx);
                                        if occupant.side() == soldier.side() { 
                                            ally_fire_count += 1.0 * visibility_multiplier; 
                                        } else { 
                                            enemy_fire_count += 1.0 * visibility_multiplier; 
                                        }
                                    }
                                }
                                break;
                            }
                        }
                    }
                }
            }
        }

        // 2. 전차 포격(Cannon Blasts) 카운트 (거리 반비례 및 시야 차단 적용)
        for blast in self.battle_state.cannon_blasts() {
            if blast.effective(*self.battle_state.frame_i()) {
                let dist_to_shooter = distance_between_points(&soldier.world_point(), blast.point());
                let meters = dist_to_shooter.meters() as f32;
                if meters <= 50.0 {
                    let visibility = Visibility::between_points(&self.config, &soldier.world_point(), blast.point(), self.battle_state.map());
                    let visibility_multiplier = if visibility.blocked { 0.5 } else { 1.0 };
                    
                    // [거리 반비례 가중치] 가까울수록 2차 곡선으로 위협 증가 (최대 20배)
                    let distance_weight = 20.0 * (1.0 - (meters / 50.0)).powi(2);
                    let threat_score = distance_weight * visibility_multiplier;

                    for vehicle in self.battle_state.vehicles() {
                        if distance_between_points(&vehicle.world_point(), blast.point()).meters() < 5 {
                            if let Some(board) = self.battle_state.vehicle_board().get(vehicle.uuid()) {
                                if let Some((_, occupant_idx)) = board.first() {
                                    let occupant = self.battle_state.soldier(*occupant_idx);
                                    if occupant.side() == soldier.side() { 
                                        ally_fire_count += threat_score; 
                                    } else { 
                                        enemy_fire_count += threat_score; 
                                    }
                                }
                            }
                            break;
                        }
                    }
                }
            }
        }

        // 3. 근접 폭발(Explosions) 자체의 공포 카운트 (거리 반비례 및 시야 차단 적용)
        for explosion in self.battle_state.explosions() {
            if explosion.effective(*self.battle_state.frame_i()) {
                let dist_to_explosion = distance_between_points(&soldier.world_point(), explosion.point());
                let meters = dist_to_explosion.meters() as f32;
                if meters <= 50.0 {
                    let visibility = Visibility::between_points(&self.config, &soldier.world_point(), explosion.point(), self.battle_state.map());
                    let visibility_multiplier = if visibility.blocked { 0.5 } else { 1.0 };

                    let distance_weight = 20.0 * (1.0 - (meters / 50.0)).powi(2);
                    let threat_score = distance_weight * visibility_multiplier;

                    enemy_fire_count += threat_score; 
                }
            }
        }

        // [로그 스케일링] 극단적인 화력 차이(15배, 50배) 상황에서도 스트레스가 무한 폭증하지 않고 수학적으로 완만하게 억제됩니다.
        let fire_ratio = enemy_fire_count / ally_fire_count;
        let log_multiplier = if fire_ratio >= 1.0 {
            1.0 + fire_ratio.log2() // 적 우세: 비율 1배=1배, 8배=4배 증가
        } else {
            1.0 / (1.0 + (1.0 / fire_ratio).log2()) // 아군 우세: 비율 0.125배=0.25배 감소
        };

        final_stress = (final_stress as f32 * log_multiplier) as u32;

        vec![RunnerMessage::BattleState(BattleStateMessage::Soldier(
            soldier_index,
            SoldierMessage::IncreaseUnderFire(final_stress),
        ))]
    }
}
