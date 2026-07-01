use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::thread;

use chrono::Local;
use battle_core::game::Side;
use battle_core::types::{GridPoint, SoldierIndex};

#[derive(Clone)]
pub struct CanvasEventLog {
    pub frame: u64,
    pub side: Side,
    pub squad_id: usize,
    pub event_type: String,
    pub sector: String,
    pub desc: String,
    pub target_squad: Option<usize>,
}

pub struct BattleLogger {
    base_dir: PathBuf,
    current_phase: usize,
    start_time: u64,
    current_order_id: Option<usize>,
    
    movements: HashMap<usize, Vec<String>>,
    movement_distances: HashMap<usize, f32>,
    engagements: HashMap<usize, Vec<String>>,
    squad_threat_scores: HashMap<usize, f32>,
    deaths_a: Vec<String>,
    deaths_b: Vec<String>,
    ammo_a: usize,
    ammo_b: usize,
    
    total_deaths_a: usize,
    total_deaths_b: usize,
    total_ammo_a: usize,
    total_ammo_b: usize,
    
    order_counter: usize,
    squad_pages: HashSet<String>,
    squad_sides: HashMap<usize, Side>,
    squad_sizes: HashMap<usize, usize>,
    
    // 캔버스 생성을 위한 지원 요청 및 할당 데이터 수집
    support_requests: HashMap<usize, Vec<SupportRequestLog>>,
    support_assignments: HashMap<usize, Vec<SupportAssignmentLog>>,
    support_supplies: HashMap<usize, Vec<SupportSupplyLog>>,
    canvas_events: Vec<CanvasEventLog>, // 전황 지도용 이벤트 로그
    canvas_node_counter: usize,
    canvas_edge_counter: usize,
    index_nodes: Vec<serde_json::Value>,
    index_edges: Vec<serde_json::Value>,
}

// 지원 요청 로그 구조체
#[derive(Clone)]
pub struct SupportRequestLog {
    pub frame: u64,
    pub requester_squad: usize,
    pub threat_level: f32,
    pub urgency: String,
    pub target_squad: Option<usize>,
    pub target_position_x: f32,
    pub target_position_y: f32,
}

// 지원 할당 로그 구조체
#[derive(Clone)]
pub struct SupportAssignmentLog {
    pub frame: u64,
    pub requester_squad: usize,
    pub supporter_squad: usize,
    pub status: String,
    pub safe_position_x: f32,
    pub safe_position_y: f32,
}

// 지원 보급 로그 구조체
#[derive(Clone)]
pub struct SupportSupplyLog {
    pub frame: u64,
    pub squad: usize,
    pub ammo_amount: usize,
    pub stress_reduction: u32,
    pub threat_before: f32,
    pub threat_after: f32,
}

fn format_squad(squad_id: usize, side: Side) -> String {
    format!("{}_Squad_{}", side, squad_id)
}

fn format_squad_link(squad_id: usize, side: Side) -> String {
    let color = match side {
        Side::A => "#4b8bdf", // 파란색
        Side::B => "#df4b4b", // 빨간색
        _ => "gray",
    };
    format!("<span style=\"color:{}\">[[{}_Squad_{}]]</span>", color, side, squad_id)
}

impl BattleLogger {
    pub fn new(start_frame: u64) -> Self {
        let now = Local::now();
        let timestamp = now.format("%Y%m%dT%H%M%S").to_string();
        let base_dir = PathBuf::from(format!("logs/{}", timestamp));
        
        fs::create_dir_all(&base_dir.join("phases")).unwrap_or_else(|e| eprintln!("Failed to create phases dir: {}", e));
        fs::create_dir_all(&base_dir.join("squads")).unwrap_or_else(|e| eprintln!("Failed to create squads dir: {}", e));
        fs::create_dir_all(&base_dir.join("orders")).unwrap_or_else(|e| eprintln!("Failed to create orders dir: {}", e));
        
        let mut logger = Self {
            base_dir,
            current_phase: 1,
            start_time: start_frame,
            current_order_id: None,
            movements: HashMap::new(),
            movement_distances: HashMap::new(),
            engagements: HashMap::new(),
            squad_threat_scores: HashMap::new(),
            deaths_a: vec![],
            deaths_b: vec![],
            ammo_a: 0,
            ammo_b: 0,
            total_deaths_a: 0,
            total_deaths_b: 0,
            total_ammo_a: 0,
            total_ammo_b: 0,
            order_counter: 0,
            squad_pages: HashSet::new(),
            squad_sides: HashMap::new(),
            squad_sizes: HashMap::new(),
            support_requests: HashMap::new(),
            support_assignments: HashMap::new(),
            support_supplies: HashMap::new(),
            canvas_events: Vec::new(),
            canvas_node_counter: 0,
            canvas_edge_counter: 0,
            index_nodes: Vec::new(),
            index_edges: Vec::new(),
        };
        
        logger.init_index();
        logger.init_obsidian_settings();
        logger
    }

    pub fn update_squad_size(&mut self, squad_id: usize, size: usize) {
        self.squad_sizes.insert(squad_id, size);
    }

    fn init_index(&mut self) {
        let title_node = serde_json::json!({
            "id": "index_title",
            "type": "text",
            "text": "# 전장 종합 기록 (Battle Logs)\n\n전체 Phase의 진행 흐름을 시각적으로 확인합니다.",
            "x": 0.0,
            "y": -300.0,
            "width": 600.0,
            "height": 150.0,
            "color": "3498db"
        });
        self.index_nodes.push(title_node);
        self.write_index_canvas();
    }

    fn write_index_canvas(&mut self) {
        let mut total_a_squads = 0;
        let mut total_b_squads = 0;
        let mut total_a_soldiers = 0;
        let mut total_b_soldiers = 0;
        for (squad_id, side) in &self.squad_sides {
            let size = *self.squad_sizes.get(squad_id).unwrap_or(&0);
            if *side == Side::A { 
                total_a_squads += 1; 
                total_a_soldiers += size;
            }
            if *side == Side::B { 
                total_b_squads += 1; 
                total_b_soldiers += size;
            }
        }

        let title_text = format!("# 전장 종합 기록 (Battle Logs)\n\n전체 Phase의 진행 흐름을 시각적으로 확인합니다.\n\n**🟦 Side A 총계**\n- 누적 참여 분대: {}개 ({}명)\n- 누적 전사자: {}명\n\n**🟥 Side B 총계**\n- 누적 참여 분대: {}개 ({}명)\n- 누적 전사자: {}명", total_a_squads, total_a_soldiers, self.total_deaths_a, total_b_squads, total_b_soldiers, self.total_deaths_b);

        if let Some(title_node) = self.index_nodes.iter_mut().find(|n| n["id"] == "index_title") {
            title_node["text"] = serde_json::json!(title_text);
        }

        let index_path = self.base_dir.join("index.canvas");
        let canvas_json = serde_json::json!({
            "nodes": self.index_nodes,
            "edges": self.index_edges
        });
        if let Ok(canvas_string) = serde_json::to_string_pretty(&canvas_json) {
            let _ = fs::write(&index_path, canvas_string);
        }
    }

    fn init_obsidian_settings(&self) {
        let obsidian_dir = self.base_dir.join(".obsidian");
        let _ = fs::create_dir_all(&obsidian_dir);
        
        // 그래프 뷰 노드 자동 컬러링을 위한 graph.json 환경 파일 생성
        let graph_json_path = obsidian_dir.join("graph.json");
        if !graph_json_path.exists() {
            let graph_json_content = r#"{
  "colorGroups": [
    {
      "query": "tag:#Side_A OR file:A_",
      "color": {
        "a": 1.0,
        "rgb": 4951007
      }
    },
    {
      "query": "tag:#Side_B OR file:B_",
      "color": {
        "a": 1.0,
        "rgb": 14633803
      }
    }
  ]
}"#;
            let _ = fs::write(graph_json_path, graph_json_content);
        }
    }

    pub fn log_movement(&mut self, frame: u64, side: Side, soldier: SoldierIndex, from_sector: String, to_sector: String, terrain: &str, is_indoor: bool, dist_m: f32, posture: &str) {
        self.squad_sides.insert(soldier.0, side);
        // 섹터 출력 여부와 상관없이 실제 병사가 이동한 미세 거리는 매 타일 이동마다 계속 누적 합산합니다.
        let current_dist = self.movement_distances.entry(soldier.0).or_insert(0.0);
        *current_dist += dist_m;
        
        // 병사가 위치한 섹터(알파벳+숫자)가 달라졌을 때만 마크다운 로그에 한 줄로 압축하여 출력합니다.
        if from_sector != to_sector {
            let env_str = if is_indoor { "실내" } else { "실외" };
            let order_link = if let Some(order_id) = self.current_order_id {
                format!("[[Order_{}]]", order_id)
            } else {
                "N/A".to_string()
            };
            let squad_link = format_squad_link(soldier.0, side);
            
            // [개선] Order 및 Squad 위키링크 추가
            let entry = format!(
                "- [Frame {}] {}: 섹터 이동: {} -> {} (진입 지형: {}, 환경: {}, 자세: {}, 누적 이동 거리: {:.1}m) (명령: {})",
                frame, squad_link, from_sector, to_sector, terrain, env_str, posture, *current_dist, order_link
            );
            self.movements.entry(soldier.0).or_insert_with(Vec::new).push(entry.clone());
            
            self.canvas_events.push(CanvasEventLog {
                frame,
                side,
                squad_id: soldier.0,
                event_type: "Movement".to_string(),
                sector: to_sector.clone(),
                desc: format!("🏃 이동: {} -> {}", from_sector, to_sector),
                target_squad: None,
            });

            // [개선] 해당 분대 페이지에도 동일 로그 추가 (양방향 링크)
            let squad_file = self.base_dir
                .join("squads")
                .join(format!("{}.md", format_squad(soldier.0, side)));
            
            if let Ok(mut file) = OpenOptions::new().append(true).create(true).open(&squad_file) {
                writeln!(file, "{}", entry).unwrap_or_default();
            }
        }
    }

    pub fn log_engagement(&mut self, frame: u64, side: Side, soldier: SoldierIndex, target_side: Side, target_squad: usize, target_grid: GridPoint, target_sector: &str, target_count: usize, target_terrain: &str, target_is_indoor: bool, posture: &str, threat_score: f32) {
        self.squad_sides.insert(soldier.0, side);
        self.squad_sides.insert(target_squad, target_side);

        let env_str = if target_is_indoor { "실내" } else { "실외" };
        let order_link = if let Some(order_id) = self.current_order_id {
            format!("[[Order_{}]]", order_id)
        } else {
            "N/A".to_string()
        };
        let squad_link = format_squad_link(soldier.0, side);
        let target_squad_link = format_squad_link(target_squad, target_side);
        
        // [개선] Order 및 양측 분대 위키링크 추가 (교전 관계 명시)
        let entry = format!(
            "- [Frame {}] {} → {} 교전: 대상 위치: {} (섹터: {}), 병력: {}명 (지형: {}, 환경: {}, 자세: {}) [위협도: {:.1}] (명령: {})",
            frame, squad_link, target_squad_link, target_grid, target_sector, target_count, target_terrain, env_str, posture, threat_score, order_link
        );
        self.engagements.entry(soldier.0).or_insert_with(Vec::new).push(entry.clone());
        *self.squad_threat_scores.entry(soldier.0).or_insert(0.0) += threat_score;

        self.canvas_events.push(CanvasEventLog {
            frame,
            side,
            squad_id: soldier.0,
            event_type: "Engagement".to_string(),
            sector: target_sector.to_string(),
            desc: format!("💥 교전 개시\n목표: [[{}]] ({}명)\n위협도: {:.1}", format_squad(target_squad, target_side), target_count, threat_score),
            target_squad: Some(target_squad),
        });
        
        // [개선] 공격자 분대 페이지에 교전 기록 추가
        let attacker_squad_file = self.base_dir
            .join("squads")
            .join(format!("{}.md", format_squad(soldier.0, side)));
        
        if let Ok(mut file) = OpenOptions::new().append(true).create(true).open(&attacker_squad_file) {
            writeln!(file, "{}", entry).unwrap_or_default();
        }
        
        // [개선] 피공격자 분대 페이지에도 교전 기록 추가 (양방향)
        let defender_squad_file = self.base_dir
            .join("squads")
            .join(format!("{}.md", format_squad(target_squad, target_side)));
        
        // 피공격자 분대 페이지가 없으면 생성
        self.ensure_squad_page(target_squad, target_side);
        
        if let Ok(mut file) = OpenOptions::new().append(true).create(true).open(&defender_squad_file) {
            let defender_entry = format!(
                "- [Frame {}] {} 에게 교전당함 (공격자: {})",
                frame, target_squad_link, squad_link
            );
            writeln!(file, "{}", defender_entry).unwrap_or_default();
        }
    }

    pub fn log_death(&mut self, frame: u64, side: Side, soldier: SoldierIndex, dead_grid: GridPoint, dead_sector: &str, dead_terrain: &str, dead_is_indoor: bool, cause: &str) {
        let env_str = if dead_is_indoor { "실내" } else { "실외" };
        let squad_link = format_squad_link(soldier.0, side);
        let entry = format!("- [Frame {}] 병사 {} 사망 | 위치: {} (섹터: {}) (지형: {}, 환경: {}) | 원인: {}", frame, squad_link, dead_grid, dead_sector, dead_terrain, env_str, cause);
        
        self.canvas_events.push(CanvasEventLog {
            frame,
            side,
            squad_id: soldier.0,
            event_type: "Death".to_string(),
            sector: dead_sector.to_string(),
            desc: format!("💀 사망\n원인: {}", cause),
            target_squad: None,
        });

        match side {
            Side::A => {
                self.deaths_a.push(entry);
                self.total_deaths_a += 1;
            },
            Side::B => {
                self.deaths_b.push(entry);
                self.total_deaths_b += 1;
            },
            _ => {}
        }
    }

    pub fn log_ammo(&mut self, side: Side, amount: usize) {
        match side {
            Side::A => {
                self.ammo_a += amount;
                self.total_ammo_a += amount;
            },
            Side::B => {
                self.ammo_b += amount;
                self.total_ammo_b += amount;
            },
            _ => {}
        }
    }

    pub fn log_support_request(
        &mut self,
        frame: u64,
        requester_squad: usize,
        threat_level: f32,
        urgency: &str,
        target_squad: Option<usize>,
        target_position: &battle_core::types::WorldPoint,
    ) {
        let entry = SupportRequestLog {
            frame,
            requester_squad,
            threat_level,
            urgency: urgency.to_string(),
            target_squad,
            target_position_x: target_position.x,
            target_position_y: target_position.y,
        };
        self.support_requests
            .entry(self.current_phase)
            .or_insert_with(Vec::new)
            .push(entry);
    }

    pub fn log_support_assignment(
        &mut self,
        frame: u64,
        requester_squad: usize,
        supporter_squad: usize,
        status: &str,
        safe_position: &battle_core::types::WorldPoint,
    ) {
        let entry = SupportAssignmentLog {
            frame,
            requester_squad,
            supporter_squad,
            status: status.to_string(),
            safe_position_x: safe_position.x,
            safe_position_y: safe_position.y,
        };
        self.support_assignments
            .entry(self.current_phase)
            .or_insert_with(Vec::new)
            .push(entry);
    }

    pub fn log_support_supply(
        &mut self,
        frame: u64,
        squad: usize,
        ammo_amount: usize,
        stress_reduction: u32,
        threat_before: f32,
        threat_after: f32,
    ) {
        let entry = SupportSupplyLog {
            frame,
            squad,
            ammo_amount,
            stress_reduction,
            threat_before,
            threat_after,
        };
        self.support_supplies
            .entry(self.current_phase)
            .or_insert_with(Vec::new)
            .push(entry);
    }

    pub fn flush_phase(&mut self, end_frame: u64, trigger: &str) {
        let phase_file = self.base_dir
            .join("phases")
            .join(format!("Phase_{}.md", self.current_phase));
        let canvas_file = self.base_dir
            .join("phases")
            .join(format!("Phase_{}.canvas", self.current_phase));
        
        let current_phase = self.current_phase;
        let start_time = self.start_time;
        let trigger_string = trigger.to_string();

        let _movements = self.movements.clone();
        let _engagements = self.engagements.clone();
        let _squad_threat_scores = self.squad_threat_scores.clone();
        let _deaths_a = self.deaths_a.clone();
        let _deaths_b = self.deaths_b.clone();
        let _ammo_a = self.ammo_a;
        let _ammo_b = self.ammo_b;
        let squad_sides = self.squad_sides.clone();
        let _squad_sizes = self.squad_sizes.clone();
        let base_dir = self.base_dir.clone();
        let current_phase_owned = current_phase;
        
        // 캔버스 생성을 위한 데이터 복사
        let support_requests_phase = self.support_requests.get(&current_phase).cloned().unwrap_or_default();
        let support_assignments_phase = self.support_assignments.get(&current_phase).cloned().unwrap_or_default();
        let support_supplies_phase = self.support_supplies.get(&current_phase).cloned().unwrap_or_default();
        let canvas_events_phase = self.canvas_events.clone();

        // index.canvas 노드/엣지 추가
        let timestamp_dir = self.base_dir.file_name().unwrap_or_default().to_string_lossy();
        let phase_node_id = format!("index_phase_{}", current_phase);
        let phase_file_path = format!("{}/phases/Phase_{}.canvas", timestamp_dir, current_phase);
        
        let y_pos = (current_phase as f32 - 1.0) * 500.0;
        
        let phase_node = serde_json::json!({
            "id": phase_node_id.clone(),
            "type": "file",
            "file": phase_file_path,
            "x": 0.0,
            "y": y_pos,
            "width": 600.0,
            "height": 400.0,
            "color": "888888"
        });
        
        self.index_nodes.push(phase_node);
        
        if current_phase > 1 {
            let prev_phase_node_id = format!("index_phase_{}", current_phase - 1);
            let edge = serde_json::json!({
                "id": format!("index_edge_{}", current_phase),
                "fromNode": prev_phase_node_id,
                "toNode": phase_node_id.clone(),
                "label": "Next Phase",
                "type": "arrow"
            });
            self.index_edges.push(edge);
        }
        
        self.write_index_canvas();

        thread::spawn(move || {
            /* // phase_N.md 생성 로직 주석 처리
            let mut content = String::new();
            
            let mut squads_in_phase_fm: Vec<String> = _movements.keys()
                .chain(_engagements.keys())
                .map(|s| {
                    let s_side = squad_sides.get(s).unwrap_or(&Side::All);
                    format!("\"[[{}]]\"", format_squad(*s, *s_side))
                })
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            squads_in_phase_fm.sort();
            let squads_fm_str = squads_in_phase_fm.join(", ");

            let mut squads_in_phase_body: Vec<String> = _movements.keys()
                .chain(_engagements.keys())
                .map(|s| {
                    let s_side = squad_sides.get(s).unwrap_or(&Side::All);
                    format_squad_link(*s, *s_side)
                })
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            squads_in_phase_body.sort();
            let squads_body_str = squads_in_phase_body.join(", ");
            
            content.push_str("---\n");
            content.push_str(&format!("phase: {}\n", current_phase_owned));
            content.push_str(&format!("start_frame: {}\n", start_time));
            content.push_str(&format!("end_frame: {}\n", end_frame));
            content.push_str(&format!("trigger_event: \"{}\"\n", trigger_string));
            content.push_str(&format!("squads: [{}]\n", squads_fm_str));
            content.push_str("---\n\n");

            content.push_str(format!("# Phase {}\n\n", current_phase_owned).as_str());
            content.push_str("[[../index|상위 디렉토리(Index)로 돌아가기]]\n\n");
            content.push_str(&format!("## 참여 분대\n{}\n\n", squads_body_str));

            content.push_str("## 지휘관 동선 및 교전 기록\n");

            if _movements.is_empty() && _engagements.is_empty() {
                content.push_str("- 해당 페이즈에 기록된 동선 및 교전이 없습니다.\n");
            } else {
                let mut keys: Vec<&usize> = _movements.keys().chain(_engagements.keys()).collect();
                keys.sort();
                keys.dedup();
                keys.sort_by(|a, b| {
                    let score_a = _squad_threat_scores.get(*a).unwrap_or(&0.0);
                    let score_b = _squad_threat_scores.get(*b).unwrap_or(&0.0);
                    score_b.partial_cmp(score_a).unwrap_or(std::cmp::Ordering::Equal)
                });
                
                for &squad_leader_id in keys {
                    let s_side = squad_sides.get(&squad_leader_id).unwrap_or(&Side::All);
                    let squad_link = format_squad_link(squad_leader_id, *s_side);
                    content.push_str(&format!("\n### {} 동선 및 교전\n", squad_link));
                    if let Some(moves) = _movements.get(&squad_leader_id) {
                        for m in moves {
                            content.push_str(&format!("{}\n", m));
                        }
                    }
                    if let Some(engs) = _engagements.get(&squad_leader_id) {
                        for e in engs {
                            content.push_str(&format!("{}\n", e));
                        }
                    }
                }
            }
            
            content.push_str("\n## 전투 손실 (시간순)\n");
            content.push_str("### 우리팀 (Side A)\n");
            if _deaths_a.is_empty() {
                content.push_str("- 사상자 없음\n");
            } else {
                for d in &_deaths_a {
                    content.push_str(&format!("{}\n", d));
                }
            }
            
            content.push_str("\n### 상대팀 (Side B)\n");
            if _deaths_b.is_empty() {
                content.push_str("- 사상자 없음\n");
            } else {
                for d in &_deaths_b {
                    content.push_str(&format!("{}\n", d));
                }
            }
            
            content.push_str("\n## 탄약 소모\n");
            content.push_str(&format!("- 우리팀 (Side A) 탄약 소모량: {}\n", _ammo_a));
            content.push_str(&format!("- 상대팀 (Side B) 탄약 소모량: {}\n", _ammo_b));

            let _ = fs::write(&phase_file, content);
            */

            // 캔버스 생성 로직 (지원-보급 플로우 및 전황 지도 통합)
            if !support_requests_phase.is_empty() || !support_assignments_phase.is_empty() || !support_supplies_phase.is_empty() || !canvas_events_phase.is_empty() {
                let mut canvas_nodes = Vec::new();
                let mut canvas_edges = Vec::new();
                let mut node_counter = 0;
                let mut edge_counter = 0;
                
                let mut unique_squads: Vec<usize> = _movements.keys().chain(_engagements.keys()).cloned().collect::<HashSet<_>>().into_iter().collect();
                unique_squads.sort();

                let mut a_squads_count = 0;
                let mut b_squads_count = 0;
                let mut a_soldiers_count = 0;
                let mut b_soldiers_count = 0;
                for squad_id in &unique_squads {
                    let size = *_squad_sizes.get(squad_id).unwrap_or(&0);
                    match squad_sides.get(squad_id).unwrap_or(&Side::All) {
                        Side::A => {
                            a_squads_count += 1;
                            a_soldiers_count += size;
                        },
                        Side::B => {
                            b_squads_count += 1;
                            b_soldiers_count += size;
                        },
                        _ => {}
                    }
                }

                let phase_deaths_a = _deaths_a.len();
                let phase_deaths_b = _deaths_b.len();

                // Phase 노드 생성
                let phase_node_id = format!("phase_{}", current_phase_owned);
                let phase_node = serde_json::json!({
                    "id": phase_node_id,
                    "type": "text",
                    "text": format!("# Phase {}\n(프레임: {} ~ {})\n\n**🟦 Side A**\n- 참여 분대: {}개 ({}명)\n- 전사자: {}명\n\n**🟥 Side B**\n- 참여 분대: {}개 ({}명)\n- 전사자: {}명", current_phase_owned, start_time, end_frame, a_squads_count, a_soldiers_count, phase_deaths_a, b_squads_count, b_soldiers_count, phase_deaths_b),
                    "x": -400.0,
                    "y": -200.0,
                    "width": 350.0,
                    "height": 200.0,
                    "color": "3498db"
                });
                canvas_nodes.push(phase_node);
                node_counter += 1;
                
                // Team A 노드
                let team_a_node = serde_json::json!({
                    "id": "team_a",
                    "type": "text",
                    "text": "🟦 Side A",
                    "x": -200.0,
                    "y": -50.0,
                    "width": 120.0,
                    "height": 50.0,
                    "color": "4b8bdf"
                });
                canvas_nodes.push(team_a_node);
                node_counter += 1;
                
                // Team B 노드
                let team_b_node = serde_json::json!({
                    "id": "team_b",
                    "type": "text",
                    "text": "🟥 Side B",
                    "x": 200.0,
                    "y": -50.0,
                    "width": 120.0,
                    "height": 50.0,
                    "color": "df4b4b"
                });
                canvas_nodes.push(team_b_node);
                node_counter += 1;
                
                // Phase → Team 연결
                let edge_phase_team_a = serde_json::json!({
                    "id": format!("edge_{}", edge_counter),
                    "fromNode": phase_node_id,
                    "toNode": "team_a",
                    "label": "소속"
                });
                canvas_edges.push(edge_phase_team_a);
                edge_counter += 1;
                
                let edge_phase_team_b = serde_json::json!({
                    "id": format!("edge_{}", edge_counter),
                    "fromNode": phase_node_id,
                    "toNode": "team_b",
                    "label": "소속"
                });
                canvas_edges.push(edge_phase_team_b);
                edge_counter += 1;

                // [추가] Team 하위 분대(Squad) 노드 생성
                let mut a_y_offset = 50.0;
                let mut b_y_offset = 50.0;
                
                for squad_id in unique_squads {
                    let side = squad_sides.get(&squad_id).unwrap_or(&Side::All);
                    let is_a = *side == Side::A;
                    
                    let squad_node_id = format!("master_squad_{}", squad_id);
                    let squad_x = if is_a { -200.0 } else { 200.0 };
                    let squad_y = if is_a { 
                        let y = a_y_offset; 
                        a_y_offset += 60.0; 
                        y 
                    } else { 
                        let y = b_y_offset; 
                        b_y_offset += 60.0; 
                        y 
                    };
                    let color = if is_a { "4b8bdf" } else { "df4b4b" };
                    let parent_node = if is_a { "team_a" } else { "team_b" };
                    
                    let size = *_squad_sizes.get(&squad_id).unwrap_or(&0);
                    let squad_node = serde_json::json!({
                        "id": squad_node_id.clone(),
                        "type": "text",
                        "text": format!("[[{}]] ({}명)", format_squad(squad_id, *side), size),
                        "x": squad_x,
                        "y": squad_y,
                        "width": 120.0,
                        "height": 40.0,
                        "color": color
                    });
                    canvas_nodes.push(squad_node);
                    node_counter += 1;
                    
                    let edge_team_squad = serde_json::json!({
                        "id": format!("edge_{}", edge_counter),
                        "fromNode": parent_node,
                        "toNode": squad_node_id,
                        "label": "분대 편성"
                    });
                    canvas_edges.push(edge_team_squad);
                    edge_counter += 1;
                }
                
                // 지원 요청 처리
                let mut request_nodes: Vec<(usize, String)> = Vec::new();
                let mut y_offset = 0.0;
                
                for req in &support_requests_phase {
                    let squad_side = squad_sides.get(&req.requester_squad).unwrap_or(&Side::All);
                    let team_prefix = match squad_side {
                        Side::A => "team_a",
                        Side::B => "team_b",
                        _ => "team_a",
                    };
                    let team_x = match squad_side {
                        Side::A => -200.0,
                        Side::B => 200.0,
                        _ => 0.0,
                    };
                    
                    let squad_node_id = format!("squad_{}_phase_{}", req.requester_squad, current_phase_owned);
                    let squad_node = serde_json::json!({
                        "id": squad_node_id,
                        "type": "text",
                        "text": format!("[[{}]]\n({}진영)", format_squad(req.requester_squad, *squad_side), squad_side),
                        "x": team_x - 150.0,
                        "y": 50.0 + y_offset,
                        "width": 120.0,
                        "height": 60.0,
                        "color": match squad_side {
                            Side::A => "4b8bdf",
                            Side::B => "df4b4b",
                            _ => "888888",
                        }
                    });
                    canvas_nodes.push(squad_node);
                    node_counter += 1;
                    
                    let request_node_id = format!("request_{}_{}", req.requester_squad, req.frame);
                    let urgency_color = {
                        match req.urgency.as_str() {
                            "Critical" => "e74c3c",
                            "High" => "e67e22",
                            "Medium" => "f1c40f",
                            "Low" => "2ecc71",
                            _ => "888888",
                        }
                    };
                    let request_node = serde_json::json!({
                        "id": request_node_id,
                        "type": "text",
                        "text": format!(
                            "🚨 지원 요청\n위협도: {:.2}\n긴급도: {}\n프레임: {}",
                            req.threat_level, req.urgency, req.frame
                        ),
                        "x": team_x + 30.0,
                        "y": 50.0 + y_offset,
                        "width": 150.0,
                        "height": 80.0,
                        "color": urgency_color
                    });
                    canvas_nodes.push(request_node);
                    node_counter += 1;
                    
                    // Squad → Request 연결
                    let edge_squad_request = serde_json::json!({
                        "id": format!("edge_{}", edge_counter),
                        "fromNode": squad_node_id,
                        "toNode": request_node_id,
                        "label": "요청 생성",
                        "type": "arrow"
                    });
                    canvas_edges.push(edge_squad_request);
                    edge_counter += 1;
                    
                    // Team → Squad 연결
                    let edge_team_squad = serde_json::json!({
                        "id": format!("edge_{}", edge_counter),
                        "fromNode": team_prefix,
                        "toNode": squad_node_id,
                        "label": "소속"
                    });
                    canvas_edges.push(edge_team_squad);
                    edge_counter += 1;
                    
                    request_nodes.push((req.requester_squad, request_node_id));
                    y_offset += 100.0;
                }
                
                // 지원 할당 처리
                let mut assignment_nodes: Vec<(usize, String)> = Vec::new();
                let mut y_offset2 = 0.0;
                
                for assign in &support_assignments_phase {
                    let requester_side = squad_sides.get(&assign.requester_squad).unwrap_or(&Side::All);
                    let team_x = match requester_side {
                        Side::A => -200.0,
                        Side::B => 200.0,
                        _ => 0.0,
                    };
                    
                    let supporter_side = squad_sides.get(&assign.supporter_squad).unwrap_or(&Side::All);
                    let supporter_team_x = match supporter_side {
                        Side::A => -200.0,
                        Side::B => 200.0,
                        _ => 0.0,
                    };
                    
                    let supporter_node_id = format!("supporter_{}_phase_{}", assign.supporter_squad, current_phase_owned);
                    let supporter_node = serde_json::json!({
                        "id": supporter_node_id,
                        "type": "text",
                        "text": format!("🔄 [[{}]] (지원군)\n상태: {}", format_squad(assign.supporter_squad, *supporter_side), assign.status),
                        "x": supporter_team_x - 150.0,
                        "y": 500.0 + y_offset2,
                        "width": 150.0,
                        "height": 70.0,
                        "color": match supporter_side {
                            Side::A => "4b8bdf",
                            Side::B => "df4b4b",
                            _ => "888888",
                        }
                    });
                    canvas_nodes.push(supporter_node);
                    node_counter += 1;
                    
                    let assignment_node_id = format!("assignment_{}_{}", assign.requester_squad, assign.frame);
                    let assignment_node = serde_json::json!({
                        "id": assignment_node_id,
                        "type": "text",
                        "text": format!(
                            "📋 지원 할당\n[[{}]] → [[{}]]\n프레임: {}",
                            format_squad(assign.supporter_squad, *supporter_side), format_squad(assign.requester_squad, *requester_side), assign.frame
                        ),
                        "x": supporter_team_x + 30.0,
                        "y": 500.0 + y_offset2,
                        "width": 180.0,
                        "height": 80.0,
                        "color": "2ecc71"
                    });
                    canvas_nodes.push(assignment_node);
                    node_counter += 1;

                    // Movement 노드 추가 (기획.txt 플로우 반영)
                    let movement_node_id = format!("movement_{}_{}", assign.supporter_squad, assign.frame);
                    let movement_node = serde_json::json!({
                        "id": movement_node_id,
                        "type": "text",
                        "text": format!("🏃 기동\n목표: {:.0}, {:.0}", assign.safe_position_x, assign.safe_position_y),
                        "x": supporter_team_x + 240.0,
                        "y": 500.0 + y_offset2 - 40.0,
                        "width": 160.0,
                        "height": 60.0,
                        "color": "9b59b6"
                    });
                    canvas_nodes.push(movement_node);
                    node_counter += 1;

                    // Engagement 노드 추가 (Movement 이후 제압 사격)
                    let engagement_node_id = format!("engagement_{}_{}", assign.supporter_squad, assign.frame);
                    let engagement_node = serde_json::json!({
                        "id": engagement_node_id,
                        "type": "text",
                        "text": "💥 제압 사격 개시\n목표: 적 위협 요소",
                        "x": supporter_team_x + 430.0,
                        "y": 500.0 + y_offset2 - 40.0,
                        "width": 160.0,
                        "height": 60.0,
                        "color": "e74c3c"
                    });
                    canvas_nodes.push(engagement_node);
                    node_counter += 1;
                    
                    // 요청자 → 할당 연결
                    for (req_squad, req_node_id) in &request_nodes {
                        if *req_squad == assign.requester_squad {
                            let edge_request_assignment = serde_json::json!({
                                "id": format!("edge_{}", edge_counter),
                                "fromNode": req_node_id,
                                "toNode": assignment_node_id,
                                "label": "할당됨",
                                "type": "arrow"
                            });
                            canvas_edges.push(edge_request_assignment);
                            edge_counter += 1;
                            break;
                        }
                    }
                    
                    // 지원군 → 할당 연결
                    let edge_supporter_assignment = serde_json::json!({
                        "id": format!("edge_{}", edge_counter),
                        "fromNode": supporter_node_id,
                        "toNode": assignment_node_id,
                        "label": "할당 대상",
                        "type": "arrow"
                    });
                    canvas_edges.push(edge_supporter_assignment);
                    edge_counter += 1;

                    // 할당 → Movement 연결
                    let edge_assign_movement = serde_json::json!({
                        "id": format!("edge_{}", edge_counter),
                        "fromNode": assignment_node_id,
                        "toNode": movement_node_id,
                        "label": "기동 시작",
                        "type": "arrow"
                    });
                    canvas_edges.push(edge_assign_movement);
                    edge_counter += 1;

                    // Movement → Engagement 연결
                    let edge_movement_engagement = serde_json::json!({
                        "id": format!("edge_{}", edge_counter),
                        "fromNode": movement_node_id,
                        "toNode": engagement_node_id,
                        "label": "사격",
                        "type": "arrow"
                    });
                    canvas_edges.push(edge_movement_engagement);
                    edge_counter += 1;
                    
                    assignment_nodes.push((assign.requester_squad, assignment_node_id));
                    
                    y_offset2 += 120.0;
                }
                
                // 지원 보급 처리
                let mut y_offset3 = 0.0;
                
                for supply in &support_supplies_phase {
                    let squad_side = squad_sides.get(&supply.squad).unwrap_or(&Side::All);
                    let team_x = match squad_side {
                        Side::A => -200.0,
                        Side::B => 200.0,
                        _ => 0.0,
                    };
                    
                    let supply_node_id = format!("supply_{}_{}", supply.squad, supply.frame);
                    let supply_node = serde_json::json!({
                        "id": supply_node_id,
                        "type": "text",
                        "text": format!("📦 [[{}]] 병력 보급\n탄약: {}발", format_squad(supply.squad, *squad_side), supply.ammo_amount),
                        "x": team_x + 240.0,
                        "y": 900.0 + y_offset3,
                        "width": 160.0,
                        "height": 70.0,
                        "color": "f39c12"
                    });
                    canvas_nodes.push(supply_node);
                    node_counter += 1;

                    let outcome_node_id = format!("outcome_{}_{}", supply.squad, supply.frame);
                    let threat_reduction = supply.threat_before - supply.threat_after;
                    let outcome_node = serde_json::json!({
                        "id": outcome_node_id,
                        "type": "text",
                        "text": format!(
                            "✅ 결과 (위협 완화)\n스트레스 감소: {}\n위협도: {:.2} → {:.2} (↓{:.2})",
                            supply.stress_reduction,
                            supply.threat_before,
                            supply.threat_after,
                            threat_reduction
                        ),
                        "x": team_x + 430.0,
                        "y": 900.0 + y_offset3 - 10.0,
                        "width": 220.0,
                        "height": 90.0,
                        "color": "3498db"
                    });
                    canvas_nodes.push(outcome_node);
                    node_counter += 1;
                    
                    // 요청자 → 보급 연결
                    for (req_squad, req_node_id) in &request_nodes {
                        if *req_squad == supply.squad {
                            let edge_request_supply = serde_json::json!({
                                "id": format!("edge_{}", edge_counter),
                                "fromNode": req_node_id,
                                "toNode": supply_node_id,
                                "label": "보급 실행",
                                "type": "arrow"
                            });
                            canvas_edges.push(edge_request_supply);
                            edge_counter += 1;
                            break;
                        }
                    }
                    
                    // 할당 → 보급 연결 (해당 분대에 대한 할당이 있는 경우)
                    for (assign_squad, assign_node_id) in &assignment_nodes {
                        if *assign_squad == supply.squad {
                            let edge_assignment_supply = serde_json::json!({
                                "id": format!("edge_{}", edge_counter),
                                "fromNode": assign_node_id,
                                "toNode": supply_node_id,
                                "label": "보급 대기",
                                "type": "arrow"
                            });
                            canvas_edges.push(edge_assignment_supply);
                            edge_counter += 1;
                            break;
                        }
                    }

                    // 보급 → Outcome 연결
                    let edge_supply_outcome = serde_json::json!({
                        "id": format!("edge_{}", edge_counter),
                        "fromNode": supply_node_id,
                        "toNode": outcome_node_id,
                        "label": "상태 안정화",
                        "type": "arrow"
                    });
                    canvas_edges.push(edge_supply_outcome);
                    edge_counter += 1;
                    
                    y_offset3 += 120.0;
                }
                
                // 전황 지도 (섹터별 타임라인) 노드 및 엣지 추가 로직 (개별 섹터 캔버스 분리 및 마스터 연결)
                if !canvas_events_phase.is_empty() {
                    let mut sector_events: HashMap<String, Vec<CanvasEventLog>> = HashMap::new();
                    for ev in &canvas_events_phase {
                        sector_events.entry(ev.sector.clone()).or_insert_with(Vec::new).push(ev.clone());
                    }

                    // 마스터 캔버스에 섹터 파일들을 배치할 시작 좌표 (지원-보급 플로우 아래쪽)
                    let mut master_sector_x = -400.0;
                    let mut master_sector_y = 1200.0;

                    // 이중 차용(E0502) 에러 방지를 위해, 이벤트를 정렬하는 가변 참조 루프를 먼저 분리합니다.
                    for events in sector_events.values_mut() {
                        events.sort_by(|a, b| a.frame.cmp(&b.frame));
                    }

                    for (sector, events) in &sector_events {
                        let mut sector_nodes = Vec::new();
                        let mut sector_edges = Vec::new();
                        let mut sector_edge_counter = 0;
                        let mut squad_last_node: HashMap<usize, String> = HashMap::new();

                        // 개별 섹터 파일이므로 (0,0) 기준에서 렌더링 시작
                        let sector_x = 0.0;
                        let sector_y = 0.0;

                        // 섹터 그룹 노드
                        let sector_group_id = format!("group_{}", sector);
                        let mut sector_deaths_a = 0;
                        let mut sector_deaths_b = 0;
                        let mut engaging_squads = HashSet::new();

                        for ev in events.iter() {
                            if ev.event_type == "Death" {
                                if ev.side == Side::A { sector_deaths_a += 1; }
                                else if ev.side == Side::B { sector_deaths_b += 1; }
                            }
                            if ev.event_type == "Engagement" {
                                engaging_squads.insert(ev.squad_id);
                                if let Some(target) = ev.target_squad {
                                    engaging_squads.insert(target);
                                }
                            }
                        }

                        let mut engaging_a_count = 0;
                        let mut engaging_a_soldiers = 0;
                        let mut engaging_b_count = 0;
                        let mut engaging_b_soldiers = 0;

                        for sq in &engaging_squads {
                            let size = *_squad_sizes.get(sq).unwrap_or(&0);
                            if squad_sides.get(sq) == Some(&Side::A) {
                                engaging_a_count += 1;
                                engaging_a_soldiers += size;
                            } else if squad_sides.get(sq) == Some(&Side::B) {
                                engaging_b_count += 1;
                                engaging_b_soldiers += size;
                            }
                        }

                        let group_node = serde_json::json!({
                            "id": sector_group_id,
                            "type": "group",
                            "label": format!("Sector {} (A전사: {}명, B전사: {}명) | 교전 분대 A: {}({}명), B: {}({}명)", sector, sector_deaths_a, sector_deaths_b, engaging_a_count, engaging_a_soldiers, engaging_b_count, engaging_b_soldiers),
                            "x": sector_x - 100.0,
                            "y": sector_y - 100.0,
                            "width": 1200.0,
                            "height": 100.0 + (events.len() as f32 * 120.0).max(200.0),
                            "color": "888888",
                            "backgroundStyle": "cover"
                        });
                        sector_nodes.push(group_node);

                        let mut a_y_offset = 0.0;
                        let mut b_y_offset = 0.0;

                        for ev in events.iter() {
                            let is_a = ev.side == Side::A;
                            let node_x = if is_a { sector_x + 50.0 } else { sector_x + 650.0 };
                            let node_y = if is_a { sector_y + a_y_offset } else { sector_y + b_y_offset };

                            let node_id = format!("ev_{}_{}_{}", ev.squad_id, ev.frame, ev.event_type);

                            let color = match ev.event_type.as_str() {
                                "Engagement" => "e74c3c", // Red
                                "Death" => "000000",      // Black
                                _ => if is_a { "4b8bdf" } else { "df4b4b" },
                            };

                            let ev_squad_size = *_squad_sizes.get(&ev.squad_id).unwrap_or(&0);
                            let text = format!("**[Frame {}] [[{}]]** ({}명)\n{}", ev.frame, format_squad(ev.squad_id, ev.side), ev_squad_size, ev.desc);

                            let node = serde_json::json!({
                                "id": node_id.clone(),
                                "type": "text",
                                "text": text,
                                "x": node_x,
                                "y": node_y,
                                "width": 250.0,
                                "height": 100.0,
                                "color": color
                            });
                            sector_nodes.push(node);

                            // 같은 분대의 타임라인 연결
                            if let Some(last_node_id) = squad_last_node.get(&ev.squad_id) {
                                let edge = serde_json::json!({
                                    "id": format!("edge_sector_{}", sector_edge_counter),
                                    "fromNode": last_node_id,
                                    "toNode": node_id,
                                    "type": "arrow",
                                    "color": if is_a { "4b8bdf" } else { "df4b4b" }
                                });
                                sector_edges.push(edge);
                                sector_edge_counter += 1;
                            }
                            squad_last_node.insert(ev.squad_id, node_id.clone());

                            if is_a {
                                a_y_offset += 150.0;
                            } else {
                                b_y_offset += 150.0;
                            }
                        }

                        // 교전 엣지 연결 (같은 섹터 내부에서 교전이 발생했을 경우에만 연결됨)
                        for ev in events.iter() {
                            if ev.event_type == "Engagement" {
                                if let Some(target_squad) = ev.target_squad {
                                    let from_node_id = format!("ev_{}_{}_{}", ev.squad_id, ev.frame, ev.event_type);
                                    
                                    if let Some(to_node_id) = squad_last_node.get(&target_squad) {
                                        let edge = serde_json::json!({
                                            "id": format!("edge_eng_{}", sector_edge_counter),
                                            "fromNode": from_node_id,
                                            "toNode": to_node_id,
                                            "type": "arrow",
                                            "color": "e74c3c",
                                            "label": "교전"
                                        });
                                        sector_edges.push(edge);
                                        sector_edge_counter += 1;
                                    }
                                }
                            }
                        }

                        // 개별 섹터 캔버스 파일 생성 및 저장
                        let sector_canvas_file_name = format!("Phase_{}_{}.canvas", current_phase_owned, sector);
                        let sector_canvas_file = base_dir.join("phases").join(&sector_canvas_file_name);
                        
                        let sector_canvas_json = serde_json::json!({
                            "nodes": sector_nodes,
                            "edges": sector_edges
                        });
                        
                        if let Ok(canvas_string) = serde_json::to_string_pretty(&sector_canvas_json) {
                            let _ = fs::write(&sector_canvas_file, canvas_string);
                        }

                        let timestamp_dir = base_dir.file_name().unwrap_or_default().to_string_lossy();

                        // 마스터 캔버스에 개별 섹터 캔버스를 포함하는 File 임베딩 노드 삽입
                        let master_sector_node_id = format!("master_sector_{}", sector);
                        let master_sector_node = serde_json::json!({
                            "id": master_sector_node_id.clone(),
                            "type": "file",
                            "file": format!("{}/phases/{}", timestamp_dir, sector_canvas_file_name),
                            "x": master_sector_x,
                            "y": master_sector_y,
                            "width": 400.0,
                            "height": 400.0,
                            "color": "888888"
                        });
                        canvas_nodes.push(master_sector_node);
                        node_counter += 1;

                        // [추가] 마스터 캔버스 내에서 Side A/B 소속, 교전, 지원 링크 생성
                        let mut squads_in_sector = HashSet::new();
                        let mut cross_sector_links = HashSet::new();

                        for ev in events.iter() {
                            squads_in_sector.insert(ev.squad_id);

                            // 섹터 간 교전 링크 (타겟이 다른 섹터일 경우)
                            if ev.event_type == "Engagement" {
                                if let Some(target) = ev.target_squad {
                                    for (other_sec, other_events) in &sector_events {
                                        if other_sec != sector && other_events.iter().any(|e| e.squad_id == target) {
                                            if cross_sector_links.insert(other_sec.clone()) {
                                                let target_sector_node_id = format!("master_sector_{}", other_sec);
                                                let edge = serde_json::json!({
                                                    "id": format!("edge_{}", edge_counter),
                                                    "fromNode": master_sector_node_id.clone(),
                                                    "toNode": target_sector_node_id,
                                                    "label": "원거리 교전",
                                                    "type": "arrow",
                                                    "color": "e74c3c"
                                                });
                                                canvas_edges.push(edge);
                                                edge_counter += 1;
                                            }
                                            break;
                                        }
                                    }
                                }
                            }
                        }

                        // 분대별 섹터 진입 링크 생성
                        for squad_id in &squads_in_sector {
                            let side = squad_sides.get(squad_id).unwrap_or(&Side::All);
                            let color = if *side == Side::A { "4b8bdf" } else { "df4b4b" };
                            let squad_node_id = format!("master_squad_{}", squad_id);
                            
                            canvas_edges.push(serde_json::json!({
                                "id": format!("edge_{}", edge_counter),
                                "fromNode": squad_node_id,
                                "toNode": master_sector_node_id.clone(),
                                "label": "섹터 진입",
                                "color": color
                            }));
                            edge_counter += 1;
                        }

                        // 지원 요청/할당 노드와 섹터 매핑
                        for squad in squads_in_sector {
                            for (req_sq, req_node_id) in &request_nodes {
                                if *req_sq == squad {
                                    canvas_edges.push(serde_json::json!({
                                        "id": format!("edge_{}", edge_counter),
                                        "fromNode": req_node_id,
                                        "toNode": master_sector_node_id.clone(),
                                        "label": "요청 발생지",
                                        "type": "arrow",
                                        "color": "e74c3c"
                                    }));
                                    edge_counter += 1;
                                }
                            }
                            for (assign_sq, assign_node_id) in &assignment_nodes {
                                if *assign_sq == squad {
                                    canvas_edges.push(serde_json::json!({
                                        "id": format!("edge_{}", edge_counter),
                                        "fromNode": assign_node_id,
                                        "toNode": master_sector_node_id.clone(),
                                        "label": "파견 목적지",
                                        "type": "arrow",
                                        "color": "2ecc71"
                                    }));
                                    edge_counter += 1;
                                }
                            }
                        }

                        master_sector_x += 450.0;
                        if master_sector_x > 1000.0 {
                            master_sector_x = -400.0;
                            master_sector_y += 450.0;
                        }
                    }
                }
                
                // 최종 캔버스 JSON 생성
                let canvas_json = serde_json::json!({
                    "nodes": canvas_nodes,
                    "edges": canvas_edges
                });
                
                if let Ok(canvas_string) = serde_json::to_string_pretty(&canvas_json) {
                    let _ = fs::write(&canvas_file, canvas_string);
                }
            }
        });

        // 페이즈 전환 처리 및 카운터 리셋
        self.movements.clear();
        self.movement_distances.clear();
        self.engagements.clear();
        self.squad_threat_scores.clear();
        self.deaths_a.clear();
        self.deaths_b.clear();
        self.ammo_a = 0;
        self.ammo_b = 0;
        self.support_requests.clear();
        self.support_assignments.clear();
        self.support_supplies.clear();
        
        self.current_phase += 1;
        self.start_time = end_frame;
    }

    pub fn end_game(&mut self, end_frame: u64, reason: &str) {
        self.flush_phase(end_frame, reason);
        
        let total_dir = self.base_dir.join("total");
        fs::create_dir_all(&total_dir).unwrap_or_else(|e| eprintln!("Failed to create total dir: {}", e));
        
        let file_path = total_dir.join("total.md");
        
        let all_squads_fm: Vec<String> = self.squad_pages.iter().map(|s| format!("\"[[{}]]\"", s)).collect();
        let squads_fm_str = all_squads_fm.join(", ");

        let all_squads_body: Vec<String> = self.squad_pages.iter().map(|s| {
            let color = if s.contains("A_Squad") { "#4b8bdf" } else if s.contains("B_Squad") { "#df4b4b" } else { "gray" };
            format!("<span style=\"color:{}\">[[{}]]</span>", color, s)
        }).collect();
        let squads_body_str = all_squads_body.join(", ");
        
        let mut content = String::new();
        content.push_str("---\n");
        content.push_str("phase: total\n");
        content.push_str(&format!("end_frame: {}\n", end_frame));
        content.push_str(&format!("end_reason: \"{}\"\n", reason));
        content.push_str(&format!("total_squads: [{}]\n", squads_fm_str));
        content.push_str("---\n\n");

        content.push_str("# Total Summary\n\n");
        content.push_str("[[../index|상위 디렉토리(Index)로 돌아가기]]\n\n");
        
        content.push_str("## 참여한 모든 분대\n");
        content.push_str(&format!("{}\n\n", squads_body_str));

        content.push_str("## 종합 전투 손실\n");
        content.push_str(&format!("- 우리팀 (Side A) 총 사망자: {}\n", self.total_deaths_a));
        content.push_str(&format!("- 상대팀 (Side B) 총 사망자: {}\n", self.total_deaths_b));
        
        content.push_str("\n## 종합 탄약 소모\n");
        content.push_str(&format!("- 우리팀 (Side A) 총 탄약 소모: {}\n", self.total_ammo_a));
        content.push_str(&format!("- 상대팀 (Side B) 총 탄약 소모: {}\n", self.total_ammo_b));
        
        content.push_str("\n## 전체 전투 단계 (Phases)\n");
        for phase in 1..=self.current_phase {
            content.push_str(&format!("- [[phases/Phase_{}|Phase {}]]\n", phase, phase));
            // 캔버스 파일이 존재하면 링크 추가
            let canvas_path = self.base_dir.join("phases").join(format!("Phase_{}_support_flow.canvas", phase));
            if canvas_path.exists() {
                content.push_str(&format!("  - [[phases/Phase_{}_support_flow|Phase {} 지원-보급 플로우 (캔버스)]]\n", phase, phase));
            }
        }

        fs::write(file_path, content).unwrap_or_else(|e| eprintln!("Failed to write total log: {}", e));

        // index.canvas에 total.md 연결
        let timestamp_dir = self.base_dir.file_name().unwrap_or_default().to_string_lossy();
        let total_node_id = "index_total".to_string();
        let y_pos = (self.current_phase as f32) * 500.0;
        
        let total_node = serde_json::json!({
            "id": total_node_id.clone(),
            "type": "file",
            "file": format!("{}/total/total.md", timestamp_dir),
            "x": 0.0,
            "y": y_pos,
            "width": 600.0,
            "height": 400.0,
            "color": "e74c3c"
        });
        
        self.index_nodes.push(total_node);
        
        let prev_phase_node_id = format!("index_phase_{}", self.current_phase);
        let edge = serde_json::json!({
            "id": "index_edge_total",
            "fromNode": prev_phase_node_id,
            "toNode": total_node_id,
            "label": format!("End (Reason: {})", reason),
            "type": "arrow"
        });
        self.index_edges.push(edge);
        
        self.write_index_canvas();
    }

    pub fn start_new_order(&mut self, command: &str, squad_id: usize, side: Side, frame: u64) -> usize {
        self.order_counter += 1;
        let order_id = self.order_counter;
        self.current_order_id = Some(order_id);
        
        let squad_name = format_squad(squad_id, side);
        let squad_link = format_squad_link(squad_id, side);

        // Orders 디렉토리에 명령 페이지 생성
        let order_file = self.base_dir
            .join("orders")
            .join(format!("Order_{}.md", order_id));
        
        // YAML Frontmatter 에는 따옴표로 감싼 순수 위키링크만 사용하고,
        // 마크다운 Body 에는 색상이 들어간 HTML 태그(squad_link)를 사용합니다.
        let content = format!(
            "---\ntitle: Order #{}\ntags: [Side_{}]\nsquad: \"[[{}]]\"\ncommand: \"{}\"\nframe: {}\nphase: \"[[Phase_{}]]\"\n---\n\n# Order #{}\n\n## 실행 명령\n- 명령어: {}\n- 대상 분대: {}\n- 실행 프레임: {}\n\n## 관련 로그\n",
            order_id, side, squad_name, command, frame, self.current_phase,
            order_id, command, squad_link, frame
        );
        
        fs::write(&order_file, content).unwrap_or_else(|e| eprintln!("Failed to create order page: {}", e));
        
        // 해당 분대 페이지에 이 명령을 링크 (분대 페이지가 없으면 생성)
        self.ensure_squad_page(squad_id, side);
        
        // Phase 페이지에 이 명령 링크 추가
        self.append_to_phase(&format!("- [[Order_{}]] 실행됨 (Frame: {})", order_id, frame));
        
        order_id
    }
    
    // [신규] 분대 페이지 보장 (없으면 생성)
    fn ensure_squad_page(&mut self, squad_id: usize, side: Side) {
        let squad_name = format_squad(squad_id, side);

        // squad_pages에는 순수 문자열 이름만 저장하여 중복 및 Frontmatter 에러를 방지합니다.
        if self.squad_pages.contains(&squad_name) {
            return;
        }
        
        self.squad_pages.insert(squad_name.clone());
        let squad_file = self.base_dir
            .join("squads")
            .join(format!("{}.md", squad_name));
        
        let content = format!(
            "---\ntitle: {}\ntags: [Side_{}]\nfirst_seen: {}\n---\n\n# {}\n\n## 소속 병사\n\n## 참여한 전투 (Phases)\n\n## 실행한 명령 (Orders)\n",
            squad_name, side, self.start_time, squad_name
        );
        
        fs::write(&squad_file, content).unwrap_or_else(|e| eprintln!("Failed to create squad page: {}", e));
    }
    
    // [신규] Phase 페이지에 내용 추가
    fn append_to_phase(&self, text: &str) {
        let phase_file = self.base_dir
            .join("phases")
            .join(format!("Phase_{}.md", self.current_phase));
        
        if let Ok(mut file) = OpenOptions::new().append(true).create(true).open(&phase_file) {
            writeln!(file, "{}", text).unwrap_or_default();
        }
    }

    fn create_canvas_node(
        &mut self,
        id: &str,
        node_type: &str,
        text: &str,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        color: Option<&str>,
        file: Option<&str>,
    ) -> serde_json::Value {
        let mut node = serde_json::json!({
            "id": id,
            "type": node_type,
            "text": text,
            "x": x,
            "y": y,
            "width": width,
            "height": height,
        });
        
        if let Some(color) = color {
            node["color"] = serde_json::json!(color);
        }
        
        if let Some(file) = file {
            node["file"] = serde_json::json!(file);
        }
        
        self.canvas_node_counter += 1;
        node
    }

    fn create_canvas_edge(
        &mut self,
        from_node: &str,
        to_node: &str,
        label: Option<&str>,
        edge_type: Option<&str>,
    ) -> serde_json::Value {
        let mut edge = serde_json::json!({
            "id": format!("edge_{}", self.canvas_edge_counter),
            "fromNode": from_node,
            "toNode": to_node,
        });
        
        if let Some(label) = label {
            edge["label"] = serde_json::json!(label);
        }
        
        if let Some(edge_type) = edge_type {
            edge["type"] = serde_json::json!(edge_type);
        }
        
        self.canvas_edge_counter += 1;
        edge
    }

    fn get_squad_side_color(&self, squad_id: usize) -> &'static str {
        if let Some(side) = self.squad_sides.get(&squad_id) {
            match side {
                Side::A => "4b8bdf",
                Side::B => "df4b4b",
                _ => "888888",
            }
        } else {
            "888888"
        }
    }

    fn get_urgency_color(&self, urgency: &str) -> &'static str {
        match urgency {
            "Critical" => "e74c3c",
            "High" => "e67e22",
            "Medium" => "f1c40f",
            "Low" => "2ecc71",
            _ => "888888",
        }
    }
}