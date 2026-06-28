use battle_core::{
    message::{InputMessage, OutputMessage},
    state::battle::BattleState,
};
use crossbeam_channel::TryRecvError;

use super::{Runner, RunnerError, AsyncTacticRequest};

impl Runner {
    pub fn inputs(&mut self) -> Result<(), RunnerError> {
        puffin::profile_scope!("inputs");
        loop {
            let inputs = match self.input.try_recv() {
                Ok(message) => message,
                Err(error) => match error {
                    TryRecvError::Empty => break,
                    TryRecvError::Disconnected => return Err(RunnerError::InputChannelClosed),
                },
            };
            log::debug!("Received {} inputs : {:?}", inputs.len(), &inputs);

            for input in inputs {
                match input {
                    InputMessage::LoadDeployment(deployment) => {
                        self.battle_state.inject(&deployment)
                    }
                    InputMessage::LoadControl((a_control, b_control)) => {
                        //
                        self.battle_state
                            .update_flags_from_control(a_control, b_control);
                    }
                    InputMessage::RequireCompleteSync => {
                        self.output
                            .send(vec![OutputMessage::LoadFromCopy(self.battle_state.copy())])?;
                    }
                    InputMessage::BattleState(battle_state_message) => {
                        // 클라이언트(GUI)에서 명시적으로 전송된 상태 변경 이벤트(예: 디버그 모드 페이즈 전환, HUD Begin 등)도 
                        // 옵시디언 로거가 정상적으로 가로채고, side_effect 버그도 해결할 수 있도록 중앙 react 파이프라인으로 우회시킵니다.
                        self.react(&vec![crate::runner::message::RunnerMessage::BattleState(battle_state_message)]);
                    }
                    InputMessage::ChangeConfig(change_config) => {
                        self.output
                            .send(vec![OutputMessage::ChangeConfig(change_config.clone())])?;
                        self.config.react(&change_config);
                    }
                    InputMessage::SetBattleState(copy) => {
                        //
                        self.battle_state = BattleState::from_copy(&copy, self.battle_state.map());
                        self.battle_state.resolve();
                        self.output.send(vec![OutputMessage::LoadFromCopy(copy)])?;
                    }
                    InputMessage::ChatCommand(command) => {
                        self.process_chat_command(&command);
                    }
                    InputMessage::RequestTacticSuggestions(query) => {
                        if let Some(tm) = &self.tactic_manager {
                            let results = tm.search(&query, 3);
                            self.output.send(vec![OutputMessage::TacticSuggestions(results)]).ok();
                        }
                    }
                };
            }
        }

        Ok(())
    }

    fn process_chat_command(&mut self, command: &str) {
        println!("==================================================");
        println!("[LLM Bridge] Received Command: {}", command);

        // [신규 추가: 직접 클릭 기반 명령어 LLM 바이패스 및 즉시 실행]
        // 입력 내용이 오직 클릭 기반(분대, 이동, 공격 토큰)으로만 구성되어 있다면 LLM을 생략하고 즉시 실행합니다.
        let tokens: Vec<&str> = command.split_whitespace().collect();
        let is_direct_command = !tokens.is_empty() && tokens.iter().all(|t| t.starts_with('@') || t.starts_with('&') || t.starts_with('#'));

        if is_direct_command {
            println!("[LLM Bridge] 클릭 입력 감지! LLM/NLP 로직을 우회하고 명령을 즉시 실행합니다.");
            let mut target_squads = vec![];
            let mut move_sectors = vec![];
            let mut attack_sectors = vec![];

            for t in &tokens {
                if t.starts_with('@') {
                    let s = t.trim_start_matches('@').trim_end_matches("분대");
                    if let Ok(id) = s.parse::<usize>() {
                        target_squads.push(battle_core::types::SquadUuid(id));
                    }
                } else if t.starts_with('&') {
                    move_sectors.push(t.trim_start_matches('&').to_string());
                } else if t.starts_with('#') {
                    attack_sectors.push(t.trim_start_matches('#').to_string());
                }
            }

            if target_squads.is_empty() {
                // 분대가 명시되지 않았다면 전 분대 선택 (디폴트 설정 적용)
                for squad_uuid in self.battle_state.squads().keys() {
                    let leader_idx = self.battle_state.squad(*squad_uuid).leader();
                    if self.battle_state.soldier(leader_idx).side() == &battle_core::game::Side::A {
                        target_squads.push(*squad_uuid);
                    }
                }
            }

            let map = self.battle_state.map();
            let grid_size = 30;
            let cell_width = map.tile_width() as f32 * grid_size as f32;
            let cell_height = map.tile_height() as f32 * grid_size as f32;
            
            let mut offset_x = 0.0;
            let mut offset_y = 0.0;
            if let Some(first_flag) = map.flags().first() {
                let flag_center = first_flag.position();
                offset_x = flag_center.x % cell_width - (cell_width / 2.0);
                offset_y = flag_center.y % cell_height - (cell_height / 2.0);
            }
            let chars: Vec<char> = "ABCDEFGHIJKLMNOPQRSTUVWXYZ".chars().collect();

            // 섹터명을 월드 좌표로 복원하는 컨버터
            let sector_to_point = |sector: &str| -> Option<battle_core::types::WorldPoint> {
                if sector.len() < 2 { return None; }
                let letter = sector.chars().next().unwrap();
                let number_str = &sector[1..];
                let row_idx = chars.iter().position(|&c| c == letter)?;
                let col_idx: usize = number_str.parse().ok()?;
                
                let px = (col_idx as i32 - 6) as f32 * cell_width + offset_x + cell_width / 2.0;
                let py = (row_idx as i32 - 5) as f32 * cell_height + offset_y + cell_height / 2.0;
                Some(battle_core::types::WorldPoint::new(px, py))
            };

            let move_pts: Vec<_> = move_sectors.iter().filter_map(|s| sector_to_point(s)).collect();
            let attack_pts: Vec<_> = attack_sectors.iter().filter_map(|s| sector_to_point(s)).collect();

            let mut messages_to_react = vec![];

            for sq_id in target_squads {
                let leader_idx = self.battle_state.squad(sq_id).leader();
                if self.battle_state.soldier(leader_idx).side() != &battle_core::game::Side::A {
                    continue;
                }
                
                let mut base_order = None;
                
                // 공격 위치가 있으면 마지막에 Defend(경계 태세) 오더 적용
                if let Some(atk_pt) = attack_pts.last() {
                    base_order = Some(battle_core::order::Order::Defend(battle_core::types::Angle(0.0)));
                    
                    // 해당 공격 위치까지는 포복(SneakTo)으로 진입
                    let leader = self.battle_state.soldier(leader_idx);
                    let from_grid = if let Some(mv_pt) = move_pts.last() {
                        map.grid_point_from_world_point(mv_pt)
                    } else {
                        map.grid_point_from_world_point(&leader.world_point())
                    };
                    let to_grid = map.grid_point_from_world_point(atk_pt);
                    
                    if let Some(grid_path) = battle_core::physics::path::find_path(
                        &self.config, map, &from_grid, &to_grid, true, &battle_core::physics::path::PathMode::Walk, &None
                    ) {
                        let world_path = grid_path.iter().map(|p| map.world_point_from_grid_point(*p)).collect();
                        let paths = battle_core::types::WorldPaths::new(vec![battle_core::types::WorldPath::new(world_path)]);
                        base_order = Some(battle_core::order::Order::SneakTo(paths, base_order.map(Box::new)));
                    } else {
                        // 길을 못 찾았더라도 직선거리 강제 이동 오더 배정 (Fallback)
                        let paths = battle_core::types::WorldPaths::new(vec![battle_core::types::WorldPath::new(vec![map.world_point_from_grid_point(from_grid), *atk_pt])]);
                        base_order = Some(battle_core::order::Order::SneakTo(paths, base_order.map(Box::new)));
                    }
                }

                // 이동 위치가 있으면 MoveFastTo 적용 후 Then으로 공격 위치 포복 이동 연결
                if let Some(mv_pt) = move_pts.last() {
                    let leader = self.battle_state.soldier(leader_idx);
                    let from_grid = map.grid_point_from_world_point(&leader.world_point());
                    let to_grid = map.grid_point_from_world_point(mv_pt);
                    
                    if let Some(grid_path) = battle_core::physics::path::find_path(
                        &self.config, map, &from_grid, &to_grid, true, &battle_core::physics::path::PathMode::Walk, &None
                    ) {
                        let world_path = grid_path.iter().map(|p| map.world_point_from_grid_point(*p)).collect();
                        let paths = battle_core::types::WorldPaths::new(vec![battle_core::types::WorldPath::new(world_path)]);
                        
                        base_order = Some(battle_core::order::Order::MoveFastTo(paths, base_order.map(Box::new)));
                    } else {
                        // 길을 못 찾았더라도 직선거리 강제 이동 오더 배정 (Fallback)
                        let paths = battle_core::types::WorldPaths::new(vec![battle_core::types::WorldPath::new(vec![leader.world_point(), *mv_pt])]);
                        base_order = Some(battle_core::order::Order::MoveFastTo(paths, base_order.map(Box::new)));
                    }
                }

                if let Some(order) = base_order {
                    let msg = battle_core::state::battle::message::BattleStateMessage::Soldier(
                        leader_idx,
                        battle_core::state::battle::message::SoldierMessage::SetOrder(order)
                    );
                    messages_to_react.push(crate::runner::message::RunnerMessage::BattleState(msg));
                }
            }

            if !messages_to_react.is_empty() {
                self.react(&messages_to_react);
            }

            println!("==================================================");
            return;
        }

        // 1. Mecab-ko 형태소 분석기 초기화 (순수 Rust 구현체)
        let current_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let dic_path = current_dir.join("model").join("mecab-ko-dic-rust");
        std::env::set_var("MECAB_DICDIR", dic_path.to_string_lossy().into_owned());

        let mut tokenizer = match mecab_ko::Tokenizer::new() {
            Ok(t) => t,
            Err(e) => {
                println!("[LLM Bridge] Tokenizer initialization error: {:?}", e);
                return;
            }
        };
        
        // 2. 문장(Sentence) 및 어절(띄어쓰기) 단위 분리
        let mut tactical_keywords: Vec<String> = vec![];

        let sentences: Vec<&str> = command
            .split(|c| c == '.' || c == '!' || c == '?')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

        let mut sentences_json_mapping = vec![];

        for sentence in sentences {
            let mut sentence_keywords: Vec<String> = vec![];
            let mut raw_keywords: Vec<String> = vec![];
            for word in sentence.split_whitespace() {
                raw_keywords.push(word.to_string());
            }

            for mut word in raw_keywords {
                loop {
                    let tokens = tokenizer.tokenize(&word);
                    if tokens.is_empty() {
                        break;
                    }
                    
                    let last_token = tokens.last().unwrap();
                    let last_text = last_token.surface.to_string();
                    let last_pos = last_token.pos.to_string();
                    
                    let is_tail = last_pos.starts_with('J') || last_pos.starts_with('E') || 
                                  last_pos.starts_with('X') || (last_pos.starts_with('S') && last_pos != "SN");
                    
                    if is_tail && word.ends_with(&last_text) && word.len() > last_text.len() {
                        word = word[..word.len() - last_text.len()].to_string();
                        continue;
                    }
                    
                    let is_target_pos = last_pos.starts_with("VV") || last_pos.starts_with("VA") || last_pos.starts_with("MAG");
                    
                    if is_target_pos && word.chars().count() > 1 {
                        let popped_word: String = word.chars().take(word.chars().count() - 1).collect();
                        let popped_tokens = tokenizer.tokenize(&popped_word);
                        
                        if let Some(popped_last) = popped_tokens.last() {
                            let p_last_pos = popped_last.pos.to_string();
                            let is_shuffled_to_core = p_last_pos.starts_with("NN") || p_last_pos.starts_with("XR");
                            
                            if is_shuffled_to_core {
                                word = popped_word;
                                continue;
                            }
                        }
                    }
                    break;
                }
                
                let mut current_chunk = String::new();
                for token in tokenizer.tokenize(&word) {
                    let text = token.surface.to_string();
                    let pos = token.pos.to_string();
                    
                    let is_core = pos.starts_with("NN") || pos.starts_with("NP") || pos.starts_with("NR") || 
                                  pos.starts_with("MM") || pos.starts_with("VV") || pos.starts_with("VA") || 
                                  pos.starts_with("VX") || pos.starts_with("MAG") || pos.starts_with("XR") || 
                                  pos.starts_with("SN") || pos.starts_with("SL") ||
                                  text.chars().any(|c| c.is_alphanumeric());
                    
                    if is_core {
                        current_chunk.push_str(&text);
                    } else {
                        if !current_chunk.is_empty() {
                            tactical_keywords.push(current_chunk.clone());
                            current_chunk.clear();
                        }
                    }
                }
                
                if !current_chunk.is_empty() {
                    tactical_keywords.push(current_chunk.clone());
                    sentence_keywords.push(current_chunk);
                }
            }

            sentences_json_mapping.push(serde_json::json!({
                "sentence": sentence,
                "matched_keywords": sentence_keywords
            }));
        }
        
        println!("[LLM Bridge] Extracted Keywords: {:?}", tactical_keywords);

        let integrated_mapping_json = serde_json::json!({
            "command_total": command,
            "split_analysis": sentences_json_mapping
        });
        if let Ok(json_log_string) = serde_json::to_string_pretty(&integrated_mapping_json) {
            println!("[LLM Bridge] Integrated Split Sentence-Keyword Mapping JSON:\n{}", json_log_string);
        }
        
        // 4. 전장 상황 컨텍스트(YAML) 스캔 및 생성
        let squad_keys: Vec<battle_core::types::SquadUuid> = self.battle_state.squads().keys().cloned().collect();

        // 명령에 특정 분대가 명시되었는지 검사 (ex: "@0분대", "@1분대" 등)
        let mut any_squad_mentioned = false;
        for squad_uuid in &squad_keys {
            let squad_match_keyword = format!("{}분대", squad_uuid.0);
            let tag_keyword = format!("@{}분대", squad_uuid.0);
            if tactical_keywords.contains(&squad_match_keyword) || command.contains(&tag_keyword) {
                any_squad_mentioned = true;
                break;
            }
        }

        for squad_uuid in squad_keys {
            let squad = self.battle_state.squad(squad_uuid);
            let leader = self.battle_state.soldier(squad.leader());

            if leader.side() != &battle_core::game::Side::A {
                continue;
            }

            let squad_match_keyword = format!("{}분대", squad_uuid.0);
            let tag_keyword = format!("@{}분대", squad_uuid.0);
            
            // 해당 분대가 명시되지 않았고, 전체 명령어 안에도 어떤 분대도 특정되지 않았다면 전 분대를 디폴트로 하달 (전체 선택)
            if any_squad_mentioned && !tactical_keywords.contains(&squad_match_keyword) && !command.contains(&tag_keyword) {
                continue;
            }

            println!("[LLM Bridge] [Async Pipeline] Target Squad {} Pushed to Async inference queue.", squad_match_keyword);

            let mut context_yaml = String::new();
            context_yaml.push_str("battle_context:\n");
            context_yaml.push_str("  metadata:\n");
            context_yaml.push_str(&format!("    frame_i: {}\n", self.battle_state.frame_i()));
            context_yaml.push_str(&format!("    phase: {}\n", self.battle_state.phase()));
            context_yaml.push_str("  selected_squad:\n");
            context_yaml.push_str(&format!("    squad_uuid: {}\n", squad_uuid.0));
            context_yaml.push_str(&format!("    side: {}\n", leader.side()));
            context_yaml.push_str(&format!("    world_point: {{ x: {:.1}, y: {:.1} }}\n", leader.world_point().x, leader.world_point().y));
            context_yaml.push_str(&format!("    behavior: {}\n", leader.behavior()));
            context_yaml.push_str(&format!("    under_fire_stress: {}\n", leader.under_fire().value()));

            // 메인 틱 스레드를 일체 방해하지 않고 비동기 수신용 큐로 오더 정보와 함께 인입 토스
            let req = AsyncTacticRequest {
                command: command.to_string(),
                tactical_keywords: tactical_keywords.clone(),
                squad_uuid,
                context_yaml,
            };
            if let Err(e) = self.async_llm_sender.send(req) {
                println!("[LLM Bridge] 백그라운드 스레드 유입 에러: {}", e);
            }
        }
    }

    // 비동기 응답 채널을 폴링하여 완료된 연산 결과가 캐치되었을 때만 처리하는 수신 파이프라인 메소드
    pub fn process_async_tactic_responses(&mut self) {
        while let Ok(res) = self.async_llm_receiver.try_recv() {
            let squad_match_keyword = format!("{}분대", res.squad_uuid.0);
            println!("[LLM Bridge] [Async Process] Processing Delayed Response for Squad {}", squad_match_keyword);
            
            if !self.battle_state.squads().contains_key(&res.squad_uuid) {
                println!("[LLM Bridge] 분대 정보 만료실패 건너뜀.");
                continue;
            }
            
            let squad = self.battle_state.squad(res.squad_uuid);
            let leader = self.battle_state.soldier(squad.leader());
            if !leader.alive() {
                println!("[LLM Bridge] 전사자 오더 처리 무시.");
                continue;
            }

            if let Ok(parsed_json) = serde_json::from_str::<serde_json::Value>(&res.json_str) {
                let mut fallback_input_msg = None;

                if parsed_json["status"] == "Success" {
                    if let Some(generated_message) = parsed_json.get("generated_input_message") {
                        if let Ok(input_msg) = serde_json::from_value::<InputMessage>(generated_message.clone()) {
                            fallback_input_msg = Some(input_msg);
                        }
                    }
                } else if let Some(tm) = &self.tactic_manager {
                    let emb_model = &tm.model;
                    // 자유 양식 분류 JSON 데이터에서 핵심 전술 기동 문장을 빌드하기 위해 필드들을 텍스트 풀로 결합합니다.
                    let mut action_text_pool = String::new();
                    if let Some(act) = parsed_json["action"].as_str() { action_text_pool.push_str(&format!(" {}", act)); }
                    if let Some(beh) = parsed_json["behavior"].as_str() { action_text_pool.push_str(&format!(" {}", beh)); }
                    if let Some(mov) = parsed_json["movement"].as_str() { action_text_pool.push_str(&format!(" {}", mov)); }
                    if let Some(tgt) = parsed_json["target"].as_str() { action_text_pool.push_str(&format!(" {}", tgt)); }
                    let combined_action_text = action_text_pool.trim().to_string();

                    if !combined_action_text.is_empty() {
                        println!("[LLM Bridge] [Embedding Fallback] 파싱 시도 문장 풀 추출 성공: '{}'", combined_action_text);
                        
                        // 기동 방식 결정을 위한 후보군 벡터 매칭 데이터베이스 정의
                        let action_candidates = vec![
                            ("SneakTo", "조용히 은밀 기동 진입 스닉 포복 포복이동 침투"),
                            ("MoveFastTo", "신속 급속 기동 런 전력질주 빠른이동 달려가기"),
                            ("MoveTo", "이동 간다 진격 목표 기동 도달"),
                            ("SuppressFire", "제압 제압사격 공격 타격 화력 사격"),
                            ("Defend", "방어 사수 대기 진지 방어태세"),
                            ("Hide", "매복 은폐 숨기 은엄폐 몸을숨겨"),
                        ];

                        let mut best_variant = "MoveTo";
                        let mut max_similarity = -1.0;

                        if let Ok(query_vec) = emb_model.get_embedding(&combined_action_text) {
                            for (variant_name, desc_text) in action_candidates {
                                if let Ok(cand_vec) = emb_model.get_embedding(desc_text) {
                                    let dot: f32 = query_vec.iter().zip(cand_vec.iter()).map(|(x, y)| (*x) * (*y)).sum::<f32>();
                                    let norm_a: f32 = query_vec.iter().map(|x| (*x) * (*x)).sum::<f32>().sqrt();
                                    let norm_b: f32 = cand_vec.iter().map(|x| (*x) * (*x)).sum::<f32>().sqrt();
                                    let similarity = if norm_a == 0.0 || norm_b == 0.0 { 0.0 } else { dot / (norm_a * norm_b) };
                                    
                                    println!("[LLM Bridge] - 임베딩 기동 후보 대조군 [{}] 유사도 지수: {:.4}", variant_name, similarity);
                                    if similarity > max_similarity {
                                        max_similarity = similarity;
                                        best_variant = variant_name;
                                    }
                                }
                            }
                        }

                        println!("[LLM Bridge] 임베딩 연산 최종 선택 전술 배리언트: [{}] (유사도 최댓값: {:.4})", best_variant, max_similarity);

                        // 문자열 매칭(contains)을 차단하고, timing 및 return 구어 요소를 다차원 문장 임베딩 모델의 벡터 대조로만 판별합니다.
                        let mut return_text_pool = String::new();
                        if let Some(t_val) = parsed_json["timing"].as_str() { return_text_pool.push_str(&format!(" {}", t_val)); }
                        if let Some(r_val) = parsed_json["return"].as_str() { return_text_pool.push_str(&format!(" {}", r_val)); }
                        let combined_return_text = return_text_pool.trim().to_string();

                        let mut has_return_intent = false;
                        if !combined_return_text.is_empty() {
                            if let Ok(ret_query_vec) = emb_model.get_embedding(&combined_return_text) {
                                if let Ok(ret_cand_vec) = emb_model.get_embedding("원대 복귀 귀환 회군 잠시 후 원래 진지 방어 대기 복귀하기") {
                                    let ret_dot: f32 = ret_query_vec.iter().zip(ret_cand_vec.iter()).map(|(x, y)| (*x) * (*y)).sum::<f32>();
                                    let ret_norm_a: f32 = ret_query_vec.iter().map(|x| (*x) * (*x)).sum::<f32>().sqrt();
                                    let ret_norm_b: f32 = ret_cand_vec.iter().map(|x| (*x) * (*x)).sum::<f32>().sqrt();
                                    let ret_similarity = if ret_norm_a == 0.0 || ret_norm_b == 0.0 { 0.0 } else { ret_dot / (ret_norm_a * ret_norm_b) };
                                    
                                    println!("[LLM Bridge] - 연계 복귀(then_order) 판단 코사인 유사도 지수: {:.4}", ret_similarity);
                                    if ret_similarity >= 0.35 {
                                        has_return_intent = true;
                                    }
                                }
                            }
                        }

                        let leader_pos = leader.world_point();
                        let grid_path = vec![
                            battle_core::types::WorldPoint::new(leader_pos.x, leader_pos.y),
                            battle_core::types::WorldPoint::new(leader_pos.x + 100.0, leader_pos.y + 100.0),
                        ];
                        let paths = battle_core::types::WorldPaths::new(vec![battle_core::types::WorldPath::new(grid_path)]);

                        let then_order = if has_return_intent {
                            println!("[LLM Bridge] 연계 복귀 의도 임베딩 감지: Move 이후 전환할 예약 오더(Order::Defend)를 주입합니다.");
                            Some(Box::new(battle_core::order::Order::Defend(battle_core::types::Angle(0.0))))
                        } else {
                            None
                        };

                        let order = match best_variant {
                            "SneakTo" => battle_core::order::Order::SneakTo(paths, then_order),
                            "MoveFastTo" => battle_core::order::Order::MoveFastTo(paths, then_order),
                            "SuppressFire" => battle_core::order::Order::SuppressFire(battle_core::types::WorldPoint::new(leader_pos.x + 200.0, leader_pos.y + 200.0)),
                            "Defend" => battle_core::order::Order::Defend(battle_core::types::Angle(0.0)),
                            "Hide" => battle_core::order::Order::Hide(battle_core::types::Angle(0.0)),
                            _ => battle_core::order::Order::MoveTo(paths, then_order),
                        };

                        let generated_msg = battle_core::state::battle::message::BattleStateMessage::Soldier(
                            squad.leader(),
                            battle_core::state::battle::message::SoldierMessage::SetOrder(order),
                        );
                        fallback_input_msg = Some(InputMessage::BattleState(generated_msg));
                    }
                }

                if let Some(input_msg) = fallback_input_msg {
                    println!("[LLM Bridge] 전술 프롬프트 임베딩 보정 파이프라인 연산 성공: 엔진 큐 인입을 개시합니다.");
                    if let InputMessage::BattleState(battle_state_message) = input_msg {
                        let frame_i = *self.battle_state.frame_i();
                        let side_effects = self.battle_state.react(&battle_state_message, frame_i);
                        
                        let runner_messages: Vec<super::message::RunnerMessage> = side_effects
                            .into_iter()
                            .map(|se| match se {
                                battle_core::state::battle::message::SideEffect::RefreshEntityAnimation(idx) => {
                                    super::message::RunnerMessage::BattleState(
                                        battle_core::state::battle::message::BattleStateMessage::Soldier(
                                            idx,
                                            battle_core::state::battle::message::SoldierMessage::SetBehavior(
                                                self.battle_state.soldier(idx).behavior().clone()
                                            )
                                        )
                                    )
                                },
                                battle_core::state::battle::message::SideEffect::SoldierFinishHisBehavior(idx, _then_order) => {
                                    super::message::RunnerMessage::BattleState(
                                        battle_core::state::battle::message::BattleStateMessage::Soldier(
                                            idx,
                                            battle_core::state::battle::message::SoldierMessage::ReachBehaviorStep
                                        )
                                    )
                                }
                            })
                            .collect();
                        self.react(&runner_messages);
                    }
                } else {
                    // PRD 7.1 절에 정의된 시스템 예외 상황(Failure) 등급 스키마 예외 제어 연동 규칙 적용
                    println!("[LLM Bridge] 전술 분석 오류: 지형적 불가능 또는 전술 해석 불능 명령어가 감지되었습니다.");
                    if let Some(execution_squad_id) = parsed_json["execution_squad_uuid"].as_u64() {
                        let _side = self.battle_state.squad_side(&battle_core::types::SquadUuid(execution_squad_id as usize)).clone();
                        
                        // 클라이언트 진영 무전 오류 효과음 연출 피드백 동기화 송출
                        self.output.send(vec![battle_core::message::OutputMessage::ClientState(
                            battle_core::state::client::ClientStateMessage::PlayInterfaceSound(battle_core::audio::Sound::Bip1)
                        )]).ok();
                    }
                }
            } else {
                println!("[LLM Bridge] 치명적 오류: LLM 에이전트로부터 전달된 데이터가 정형화된 JSON 포맷이 아닙니다.");
            }
        }
        
        println!("==================================================");
    }

    /// 현재 엔진의 배틀 상태(BattleState)를 스캔하여 LLM이 전술적으로 판단할 수 있는 YAML 포맷의 문자열로 반환합니다.
    pub fn build_yaml_context(&self) -> String {
        let mut ctx_str = String::new();
        ctx_str.push_str("battle_context:\n");
        ctx_str.push_str("  metadata:\n");
        ctx_str.push_str(&format!("    frame_i: {}\n", self.battle_state.frame_i()));
        ctx_str.push_str(&format!("    phase: {}\n", self.battle_state.phase()));
        ctx_str.push_str("  squads:\n");
        
        for (squad_uuid, squad) in self.battle_state.squads() {
            let leader = self.battle_state.soldier(squad.leader());
            
            // [Bonsai-4B 토큰 컨텍스트오버플로우 1024 제한 방어 필터 적용]
            // 전장에 존재하는 20개 이상의 모든 분대를 무지성으로 전송하면 1024 토큰을 초과하여 엔진이 폭발합니다.
            // 따라서 플레이어의 명령 대상인 아군 진영(Side::A) 정보와 사기/위협 상태 위주로 컨텍스트를 다이어트합니다.
            if leader.side() != &battle_core::game::Side::A && !leader.under_fire().exist() {
                continue; // 무의미하게 은엄폐 중인 아군 사선 밖의 적 진영 데이터는 프롬프트에서 생략 차단합니다.
            }

            ctx_str.push_str(&format!("    - squad_uuid: {}\n", squad_uuid.0));
            ctx_str.push_str(&format!("      side: {}\n", leader.side()));
            ctx_str.push_str(&format!("      world_point: {{ x: {:.1}, y: {:.1} }}\n", leader.world_point().x, leader.world_point().y));
            ctx_str.push_str(&format!("      behavior: {}\n", leader.behavior()));
            ctx_str.push_str(&format!("      under_fire_stress: {}\n", leader.under_fire().value()));
        }
        
        ctx_str
    }
}
