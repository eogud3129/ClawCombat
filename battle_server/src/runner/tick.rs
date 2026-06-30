use battle_core::state::battle::message::BattleStateMessage;

use crate::runner::message::RunnerMessage;

use super::{Runner, RunnerError};

impl Runner {
    pub fn tick(&mut self) -> Result<(), RunnerError> {
        let frame_i = self.battle_state.frame_i();
        puffin::profile_scope!("tick", format!("frame {frame_i}"));
        self.inputs()?;

        let mut messages = vec![RunnerMessage::BattleState(
            BattleStateMessage::IncrementFrameI,
        )];
        messages.extend(self.tick_phase());
        messages.extend(self.tick_morale());
        messages.extend(self.tick_victory());
        messages.extend(self.tick_flags());
        messages.extend(self.tick_soldiers());
        messages.extend(self.tick_update_squad_leaders());
        messages.extend(self.tick_feeling_decreasing_soldiers());
        messages.extend(self.tick_visibilities());
        messages.extend(self.tick_physics());
        
        // [Operation Ghost - Part 1] 매 60프레임(약 1초)마다 전장 섹터를 분석하고 분대를 중대로 묶는(동적 편제) 로직 호출
        if *self.battle_state.frame_i() % 60 == 0 && self.battle_state.phase().is_battle() {
            self.tick_company_grouping();

            // [Step 1: Tactical Ping] 만료된 전술 핑(위험 사로) 메모리 정리
            let current_frame = *self.battle_state.frame_i();
            self.tactical_pings.retain(|_, (expire_frame, _)| *expire_frame > current_frame);
        }

        self.react(&messages);
        self.clean();

        self.outputs(&messages)?;
        Ok(())
    }

    pub fn clean(&mut self) {
        self.battle_state.clean(None);
    }

    pub fn tick_company_grouping(&mut self) {
        let mut new_companies = std::collections::HashMap::new();
        let map = self.battle_state.map();
        
        for side in [battle_core::game::Side::A, battle_core::game::Side::B] {
            let mut side_squads = vec![];
            for (squad_uuid, squad) in self.battle_state.squads() {
                let leader = self.battle_state.soldier(squad.leader());
                
                // [Part 1 & 3 개선: 고스트 분대 정찰조 필터링 강화]
                // 새롭게 추가된 SquadComposition::is_operational() 메서드를 활용하여,
                // 분대 내에 지휘관 자격을 갖춘 인원이 1명이라도 있는지 확인하고 정상 작동하는 분대만 편제합니다.
                if leader.side() == &side && squad.is_operational(self.battle_state.soldiers()) {
                    side_squads.push(*squad_uuid);
                }
            }
            
            // [수정] 거리 기반 동적 그룹핑을 폐지하고, 맵의 Y축(상/중/하)을 기준으로 진영 내 분대를 3개의 중대로 균등 분할합니다.
            side_squads.sort_by(|a, b| {
                let a_y = self.battle_state.soldier(self.battle_state.squad(*a).leader()).world_point().y;
                let b_y = self.battle_state.soldier(self.battle_state.squad(*b).leader()).world_point().y;
                a_y.partial_cmp(&b_y).unwrap_or(std::cmp::Ordering::Equal)
            });

            let mut clusters: Vec<Vec<battle_core::types::SquadUuid>> = vec![];
            if !side_squads.is_empty() {
                // 상, 중, 하 3개 중대로 무조건 분할하되, 총 분대 수가 3개 미만이면 그 수에 맞춤
                let num_companies = 3.min(side_squads.len());
                let chunk_size = (side_squads.len() as f32 / num_companies as f32).ceil() as usize;
                for chunk in side_squads.chunks(chunk_size) {
                    clusters.push(chunk.to_vec());
                }
            }
            
            // 중대 명명 전에 각 클러스터 내부를 정렬하고, 전체 클러스터 목록을 최소 분대 UUID 기준으로 결정론적 정렬 수행
            for cluster in &mut clusters {
                cluster.sort_by(|a, b| a.0.cmp(&b.0));
            }
            clusters.sort_by(|a, b| {
                let a_val = a.first().map(|s| s.0).unwrap_or(0);
                let b_val = b.first().map(|s| s.0).unwrap_or(0);
                a_val.cmp(&b_val)
            });

            let company_names = ["Alpha", "Bravo", "Charlie", "Delta", "Echo", "Foxtrot", "Golf", "Hotel"];
            let mut side_company_names = vec![];
            
            for (i, cluster) in clusters.into_iter().enumerate() {
                let name = format!("{}-{}", side, company_names.get(i).unwrap_or(&"Omega"));
                side_company_names.push(name.clone());
                new_companies.insert(name.clone(), crate::runner::Company {
                    id: name,
                    squads: cluster,
                    scout_squad: None, // 글로벌 로테이션에서 할당됨
                    side,
                });
            }
            
            side_company_names.sort();
            
            if !side_company_names.is_empty() {
                let map = self.battle_state.map();

                // [Phase 2 적용] 각 중대별로 타겟을 분산시키기 위해 이미 선택된 목표 깃발의 페널티 가중치를 추적합니다.
                let mut targeted_flags_penalty: std::collections::HashMap<String, f32> = std::collections::HashMap::new();

                // [Phase 1 적용] 진영 단위 글로벌 중대 로테이션(GLOBAL_TURN)을 폐지하고 모든 중대가 독립적으로 정찰조를 가동합니다.
                for company_name in &side_company_names {
                    // 각 중대별로 최적의 방면 깃발을 탐색하고 개별 분대 로테이션을 가동합니다.
                    let mut best_target_flag = None;
                    let mut min_dist_to_flag = std::f32::MAX;

                    if let Some(comp) = new_companies.get(company_name) {
                        if let Some(first_squad) = comp.squads.first() {
                            let leader = self.battle_state.soldier(self.battle_state.squad(*first_squad).leader());
                            let leader_pos = leader.world_point();

                            for flag in map.flags() {
                                let is_owned = self.battle_state.flags().ownerships().iter().any(|(n, o)| {
                                    n == flag.name() && (
                                        o == &battle_core::game::flag::FlagOwnership::Both || 
                                        (side == battle_core::game::Side::A && o == &battle_core::game::flag::FlagOwnership::A) ||
                                        (side == battle_core::game::Side::B && o == &battle_core::game::flag::FlagOwnership::B)
                                    )
                                });

                                if !is_owned {
                                    let base_dist = battle_core::physics::utils::distance_between_points(&leader_pos, &flag.position()).meters() as f32;
                                    
                                    // [Phase 2 적용] 이미 다른 아군 중대가 목표로 삼고 있는 깃발이면 거리에 페널티를 부여하여 양동 작전을 유도합니다.
                                    let penalty = targeted_flags_penalty.get(&flag.name().0).unwrap_or(&0.0);
                                    let adjusted_dist = base_dist + penalty;

                                    if adjusted_dist < min_dist_to_flag {
                                        min_dist_to_flag = adjusted_dist;
                                        best_target_flag = Some(flag.clone());
                                    }
                                }
                            }
                        }
                    }

                    // 중대 고유의 최적 목표 깃발이 결정되었다면, 중대 내부의 정찰 분대 로테이션을 병렬로 가동합니다.
                    if let Some(flag) = best_target_flag {
                        // [Phase 2 적용] 다른 중대가 같은 깃발로 몰리지 않도록 페널티 가중치를 누적합니다. (예: 1개 중대당 거리 150m 증가 페널티)
                        *targeted_flags_penalty.entry(flag.name().0.clone()).or_insert(0.0) += 150.0;

                        if let Some(comp) = new_companies.get_mut(company_name) {
                            let mut sorted_squads = comp.squads.clone();
                            sorted_squads.sort_by(|a, b| a.0.cmp(&b.0));

                            // 가변적인 중대 이름 대신 내부 최소 분대 UUID를 고정 식별자로 활용하는 앵커 키 생성
                            let cluster_anchor_key = format!("{}-group-{}", side, sorted_squads[0].0);

                            // 1. 해당 중대의 과거 정찰 완료 블랙리스트 기록 조회 및 획득
                            let mut history_guard = self.scouted_history.write().unwrap();
                            let current_history = history_guard.entry(cluster_anchor_key.clone()).or_insert_with(std::collections::HashSet::new);

                            // [히스토리 관리 개선]
                            // 이미 전멸하거나 해체되어 편제(sorted_squads)에서 사라진 분대는 히스토리 맵에서도 제거하여 논리 무결성을 유지합니다.
                            current_history.retain(|sq| sorted_squads.contains(sq));

                            // 무분별한 리셋 출력을 방지하기 위해 잔여 분대 카운트를 대조하여 리셋을 단행합니다.
                            let has_fresh = sorted_squads.iter().any(|sq| !current_history.contains(sq));
                            if !has_fresh && sorted_squads.len() > 1 {
                                current_history.clear();
                                println!("[시스템 강제 로테이션] 중대 {} 내의 모든 분대가 정찰 임무를 완수했습니다. 정찰 기록 보관소를 클리어하고 대순환 주기를 리셋합니다.", company_name);
                            }

                            // [순차 로테이션 결정론적 교정]
                            // 블랙리스트 역사 보관소에 포함되지 않은 첫 번째 청정 분대를 정렬된 순서대로 탐색하여 순차 배정합니다.
                            let sq_uuid = sorted_squads
                                .iter()
                                .find(|sq| !current_history.contains(sq))
                                .cloned()
                                .unwrap_or(sorted_squads[0]);

                            comp.scout_squad = Some(sq_uuid);
                            
                            // 해당 액티브 중대 내부의 분대 교대 로테이션 지수를 개별 출력합니다.
                            let completed_count = sorted_squads.iter().filter(|sq| current_history.contains(sq)).count();
                            if *self.battle_state.frame_i() % 600 == 0 {
                                // [상태 추적 로그 개선] 정찰조 지휘관의 현재 행동 상태와 목표까지의 거리를 계산하여 함께 출력합니다.
                                let mut current_status = "대기/판단 중".to_string();
                                let mut dist_str = "".to_string();
                                
                                if let Some(scout_comp) = self.battle_state.squads().get(&sq_uuid) {
                                    let scout_leader = self.battle_state.soldier(scout_comp.leader());
                                    
                                    let dist = battle_core::physics::utils::distance_between_points(&scout_leader.world_point(), &flag.position()).meters();
                                    dist_str = format!(" (목표까지 {}m 남음)", dist);

                                    current_status = match scout_leader.order() {
                                        battle_core::order::Order::MoveTo(_, _) | battle_core::order::Order::MoveFastTo(_, _) | battle_core::order::Order::SneakTo(_, _) => "기동 중".to_string(),
                                        battle_core::order::Order::EngageSquad(_) | battle_core::order::Order::SuppressFire(_) => "적군과 교전 중".to_string(),
                                        battle_core::order::Order::Hide(_) => "은엄폐/회피 중".to_string(),
                                        battle_core::order::Order::Defend(_) => "방어/점령 대기 중".to_string(),
                                        battle_core::order::Order::Idle => "대기 중".to_string(),
                                        battle_core::order::Order::OffMapTransit(_) => "맵 밖으로 후퇴 중".to_string(),
                                    };
                                    
                                    // 심각한 스트레스(피격 공포) 상태일 경우 시각적 경고 추가
                                    if scout_leader.under_fire().is_danger() || scout_leader.under_fire().is_max() {
                                        current_status = format!("{} [위험/경직!]", current_status);
                                    }
                                }

                                println!("[시스템] 진영 {} - 깃발[{}] 방면 기동 중대: {} | 정찰 분대: {} (로테이션: {}/{}) | 현재 상태: {}{}", 
                                    side, flag.name().0, company_name, sq_uuid.0, completed_count + 1, sorted_squads.len(), current_status, dist_str);
                            }
                        }
                    }
                }
            }
        }
        
        // 최종 컨테이너 동기화 병합을 보장하여 다중 중대 동시다발 정찰조 가동 레이스를 완벽하게 성립시킵니다.
        for (name, comp) in new_companies {
            self.companies.insert(name, comp);
        }
    }
}
