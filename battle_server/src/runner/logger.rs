use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::thread;

use chrono::Local;
use battle_core::game::Side;
use battle_core::types::{GridPoint, SoldierIndex};


pub struct BattleLogger {
    base_dir: PathBuf,
    current_phase: usize,
    start_time: u64, // 기록 시작 프레임
    
    // 현재 페이즈 누적 데이터 (분대장 ID를 Key로 하여 동선 그룹화)
    movements: HashMap<usize, Vec<String>>,
    movement_distances: HashMap<usize, f32>, // 누적 이동 거리 추가
    engagements: HashMap<usize, Vec<String>>,
    squad_threat_scores: HashMap<usize, f32>, // [추가] 분대별 위협도(교전거리+공격량) 합산
    deaths_a: Vec<String>,
    deaths_b: Vec<String>,
    ammo_a: usize,
    ammo_b: usize,
    
    // 전체 게임 종합 데이터
    total_deaths_a: usize,
    total_deaths_b: usize,
    total_ammo_a: usize,
    total_ammo_b: usize,
}

impl BattleLogger {
    pub fn new(start_frame: u64) -> Self {
        let now = Local::now();
        let timestamp = now.format("%Y%m%dT%H%M%S").to_string();
        let base_dir = PathBuf::from(format!("logs/{}", timestamp));
        
        fs::create_dir_all(&base_dir).unwrap_or_else(|e| eprintln!("Failed to create log dir: {}", e));
        
        let logger = Self {
            base_dir,
            current_phase: 1,
            start_time: start_frame,
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
        };
        
        logger.init_index();
        logger.init_phase_dir();
        logger
    }

    fn init_index(&self) {
        let index_path = self.base_dir.join("index.md");
        if !index_path.exists() {
            let content = "---\ntitle: Battle Log Index\n---\n\n# 전장 종합 기록 (Battle Logs)\n\n옵시디언 위키 링크를 통해 각 페이즈(Phase)별 세부 기록을 확인할 수 있습니다.\n\n## Phases\n";
            fs::write(&index_path, content).unwrap_or_else(|e| eprintln!("Failed to create index.md: {}", e));
        }
    }

    fn append_to_index(&self, text: &str) {
        let index_path = self.base_dir.join("index.md");
        if let Ok(mut file) = OpenOptions::new().append(true).create(true).open(&index_path) {
            writeln!(file, "{}", text).unwrap_or_default();
        }
    }

    fn init_phase_dir(&self) {
        let phase_dir = self.base_dir.join(format!("phase_{}", self.current_phase));
        fs::create_dir_all(&phase_dir).unwrap_or_else(|e| eprintln!("Failed to create phase dir: {}", e));
    }

    pub fn log_movement(&mut self, frame: u64, soldier: SoldierIndex, from_sector: String, to_sector: String, terrain: &str, is_indoor: bool, dist_m: f32, posture: &str) {
        // 섹터 출력 여부와 상관없이 실제 병사가 이동한 미세 거리는 매 타일 이동마다 계속 누적 합산합니다.
        let current_dist = self.movement_distances.entry(soldier.0).or_insert(0.0);
        *current_dist += dist_m;
        
        // 병사가 위치한 섹터(알파벳+숫자)가 달라졌을 때만 마크다운 로그에 한 줄로 압축하여 출력합니다.
        if from_sector != to_sector {
            let env_str = if is_indoor { "실내" } else { "실외" };
            let entry = format!("- [Frame {}] 섹터 이동: {} -> {} (진입 지형: {}, 환경: {}, 자세: {}, 현재까지 누적 이동 거리: {:.1}m)", frame, from_sector, to_sector, terrain, env_str, posture, *current_dist);
            self.movements.entry(soldier.0).or_insert_with(Vec::new).push(entry);
        }
    }

    pub fn log_engagement(&mut self, frame: u64, soldier: SoldierIndex, target_squad: usize, target_grid: GridPoint, target_sector: &str, target_count: usize, target_terrain: &str, target_is_indoor: bool, posture: &str, threat_score: f32) {
        let env_str = if target_is_indoor { "실내" } else { "실외" };
        let entry = format!("- [Frame {}] 교전 발생: 대상 분대 [{}], 위치: {} (섹터: {}), 병력: {}명 (지형: {}, 환경: {}, 자세: {}) [위협도: {:.1}]", frame, target_squad, target_grid, target_sector, target_count, target_terrain, env_str, posture, threat_score);
        self.engagements.entry(soldier.0).or_insert_with(Vec::new).push(entry);
        *self.squad_threat_scores.entry(soldier.0).or_insert(0.0) += threat_score;
    }

    pub fn log_death(&mut self, frame: u64, side: Side, soldier: SoldierIndex, dead_grid: GridPoint, dead_sector: &str, dead_terrain: &str, dead_is_indoor: bool, cause: &str) {
        let env_str = if dead_is_indoor { "실내" } else { "실외" };
        let entry = format!("- [Frame {}] 병사 {} 사망 | 위치: {} (섹터: {}) (지형: {}, 환경: {}) | 원인: {}", frame, soldier.0, dead_grid, dead_sector, dead_terrain, env_str, cause);
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

    pub fn flush_phase(&mut self, end_frame: u64, trigger: &str) {
        let phase_dir = self.base_dir.join(format!("phase_{}", self.current_phase));
        let file_path = phase_dir.join(format!("phase_{}.md", self.current_phase));
        let summary_file_path = phase_dir.join(format!("summary_{}.md", self.current_phase));
        let index_path = self.base_dir.join("index.md");
        
        let current_phase = self.current_phase;
        let start_time = self.start_time;
        let trigger_string = trigger.to_string();

        // 메인 스레드 소유권 분리를 위해 내부 누적 컨텍스트 데이터를 깊은 복사(Clone)합니다.
        let movements = self.movements.clone();
        let engagements = self.engagements.clone();
        let squad_threat_scores = self.squad_threat_scores.clone();
        let deaths_a = self.deaths_a.clone();
        let deaths_b = self.deaths_b.clone();
        let ammo_a = self.ammo_a;
        let ammo_b = self.ammo_b;

        // 대량의 마크다운 문자열 조합 및 디스크 I/O 연산 전체를 백그라운드 스레드로 격리하여 프리징을 차단합니다.
        thread::spawn(move || {
            let mut content = String::new();
            content.push_str("---\n");
            content.push_str(&format!("phase: {}\n", current_phase));
            content.push_str(&format!("start_frame: {}\n", start_time));
            content.push_str(&format!("end_frame: {}\n", end_frame));
            content.push_str(&format!("trigger_event: \"{}\"\n", trigger_string));
            content.push_str("---\n\n");

            let mut summary_content = content.clone();

            content.push_str(format!("# Phase {}\n\n", current_phase).as_str());
            content.push_str("[[../index|상위 디렉토리(Index)로 돌아가기]]\n\n");

            summary_content.push_str(format!("# Summary Phase {}\n\n", current_phase).as_str());
            summary_content.push_str("[[../index|상위 디렉토리(Index)로 돌아가기]]\n\n");

            content.push_str("## 지휘관 동선 및 교전 기록\n");
            summary_content.push_str("## 지휘관 동선 및 교전 요약 기록\n");

            if movements.is_empty() && engagements.is_empty() {
                content.push_str("- 해당 페이즈에 기록된 동선 및 교전이 없습니다.\n");
                summary_content.push_str("- 해당 페이즈에 기록된 동선 및 교전이 없습니다.\n");
            } else {
                let mut keys: Vec<&usize> = movements.keys().chain(engagements.keys()).collect();
                keys.sort();
                keys.dedup();
                keys.sort_by(|a, b| {
                    let score_a = squad_threat_scores.get(*a).unwrap_or(&0.0);
                    let score_b = squad_threat_scores.get(*b).unwrap_or(&0.0);
                    score_b.partial_cmp(score_a).unwrap_or(std::cmp::Ordering::Equal)
                });
                
                for &squad_leader_id in keys {
                    content.push_str(&format!("\n### 분대장 [{}] 동선 및 교전\n", squad_leader_id));
                    if let Some(moves) = movements.get(&squad_leader_id) {
                        for m in moves {
                            content.push_str(&format!("{}\n", m));
                        }
                    }
                    if let Some(engs) = engagements.get(&squad_leader_id) {
                        for e in engs {
                            content.push_str(&format!("{}\n", e));
                        }
                        if let Some(first_eng) = engs.first() {
                            summary_content.push_str(&format!("\n### 분대장 [{}] 동선 및 교전\n", squad_leader_id));
                            summary_content.push_str(&format!("{}\n", first_eng));
                        }
                    }
                }
            }
            
            content.push_str("\n## 전투 손실 (시간순)\n");
            content.push_str("### 우리팀 (Side A)\n");
            if deaths_a.is_empty() {
                content.push_str("- 사상자 없음\n");
            } else {
                for d in &deaths_a {
                    content.push_str(&format!("{}\n", d));
                }
            }
            
            content.push_str("\n### 상대팀 (Side B)\n");
            if deaths_b.is_empty() {
                content.push_str("- 사상자 없음\n");
            } else {
                for d in &deaths_b {
                    content.push_str(&format!("{}\n", d));
                }
            }
            
            content.push_str("\n## 탄약 소모\n");
            content.push_str(&format!("- 우리팀 (Side A) 탄약 소모량: {}\n", ammo_a));
            content.push_str(&format!("- 상대팀 (Side B) 탄약 소모량: {}\n", ammo_b));

            let _ = fs::write(file_path, content);
            let _ = fs::write(summary_file_path, summary_content);

            if let Ok(mut file) = OpenOptions::new().append(true).create(true).open(&index_path) {
                let _ = writeln!(file, "- [[phase_{}/phase_{}|Phase {} 기록 보기]] (Trigger: {})", current_phase, current_phase, current_phase, trigger_string);
                let _ = writeln!(file, "  - [[phase_{}/summary_{}|Phase {} 요약 보기]]", current_phase, current_phase, current_phase);
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
        
        self.current_phase += 1;
        self.start_time = end_frame;
        self.init_phase_dir();
    }

    pub fn end_game(&mut self, end_frame: u64, reason: &str) {
        self.flush_phase(end_frame, reason);
        
        let total_dir = self.base_dir.join("total");
        fs::create_dir_all(&total_dir).unwrap_or_else(|e| eprintln!("Failed to create total dir: {}", e));
        
        let file_path = total_dir.join("total.md");
        
        let mut content = String::new();
        content.push_str("---\n");
        content.push_str("phase: total\n");
        content.push_str(&format!("end_frame: {}\n", end_frame));
        content.push_str(&format!("end_reason: \"{}\"\n", reason));
        content.push_str("---\n\n");

        content.push_str("# Total Summary\n\n");
        content.push_str("[[../index|상위 디렉토리(Index)로 돌아가기]]\n\n");

        content.push_str("## 종합 전투 손실\n");
        content.push_str(&format!("- 우리팀 (Side A) 총 사망자: {}\n", self.total_deaths_a));
        content.push_str(&format!("- 상대팀 (Side B) 총 사망자: {}\n", self.total_deaths_b));
        
        content.push_str("\n## 종합 탄약 소모\n");
        content.push_str(&format!("- 우리팀 (Side A) 총 탄약 소모: {}\n", self.total_ammo_a));
        content.push_str(&format!("- 상대팀 (Side B) 총 탄약 소모: {}\n", self.total_ammo_b));

        fs::write(file_path, content).unwrap_or_else(|e| eprintln!("Failed to write total log: {}", e));

        // 옵시디언 index.md 에 Total 위키 링크 추가
        self.append_to_index("\n## 종합 결과");
        self.append_to_index(&format!("- [[total/total|전체 종합 기록 보기]] (Reason: {})", reason));
    }
}