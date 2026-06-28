use battle_core::{
    config::ServerConfig,
    message::{InputMessage, OutputMessage},
    state::battle::BattleState,
};
use crossbeam_channel::{Receiver, SendError, Sender};
use std::{
    fmt::Display,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use llm_agent::generate::LlmGenerator;

pub mod embedding;
pub mod logger;
mod behavior;
mod engage;
mod fight;
mod flag;
mod gesture;
mod input;
mod message;
mod morale;
mod movement;
mod output;
mod phase;
mod physics;
mod react;
mod soldier;
mod tick;
mod update;
mod utils;
mod vehicle;
mod victory;
mod visibility;

#[derive(Debug, Clone)]
pub struct Company {
    pub id: String,
    pub squads: Vec<battle_core::types::SquadUuid>,
    pub scout_squad: Option<battle_core::types::SquadUuid>,
    pub side: battle_core::game::Side,
}

pub struct AsyncTacticRequest {
    pub command: String,
    pub tactical_keywords: Vec<String>,
    pub squad_uuid: battle_core::types::SquadUuid,
    pub context_yaml: String,
}

pub struct AsyncTacticResponse {
    pub squad_uuid: battle_core::types::SquadUuid,
    pub json_str: String,
    pub tactical_keywords: Vec<String>,
    pub command: String,
}

pub struct Runner {
    config: ServerConfig,
    input: Receiver<Vec<InputMessage>>,
    output: Sender<Vec<OutputMessage>>,
    stop_required: Arc<AtomicBool>,
    last: Instant,
    current_visibility: usize,
    battle_state: BattleState,
    #[allow(dead_code)]
    llm_agent: LlmGenerator,
    #[allow(dead_code)]
    pub tactic_manager: Option<embedding::TacticManager>,
    pub logger: Option<logger::BattleLogger>,
    pub companies: std::collections::HashMap<String, Company>,
    // [Step 1: Tactical Ping] 적군 사격 원점을 기억하는 전술 메모리 (좌표, (만료 프레임, 진영))
    pub tactical_pings: std::collections::HashMap<battle_core::types::GridPoint, (u64, battle_core::game::Side)>,
    // [전술 체크포인트 메모리] 깃발 진입 전 안전 구역 좌표를 기억하여 점령/후퇴 시 복귀하는 용도
    pub checkpoints: std::sync::RwLock<std::collections::HashMap<battle_core::types::SquadUuid, battle_core::types::WorldPoint>>,
    // [스마트 로테이션 메모리] 중대별로 강제로 턴을 넘긴 횟수와 마지막 교대 프레임을 기록합니다.
    pub scout_turn_offsets: std::sync::RwLock<std::collections::HashMap<String, (usize, u64)>>,
    // [강제 로테이션 이력 스토리지] 중대별로 이미 정찰조 임무를 수행한 분대 리스트를 기억하여 독점을 원천 차단합니다.
    pub scouted_history: std::sync::RwLock<std::collections::HashMap<String, std::collections::HashSet<battle_core::types::SquadUuid>>>,
    // 비동기 LLM 전술 요청 전송 채널
    pub async_llm_sender: Sender<AsyncTacticRequest>,
    // 비동기 LLM 결과 수신 채널
    pub async_llm_receiver: Receiver<AsyncTacticResponse>,
}

impl Runner {
    pub fn new(
        config: ServerConfig,
        input: Receiver<Vec<InputMessage>>,
        output: Sender<Vec<OutputMessage>>,
        stop_required: Arc<AtomicBool>,
        state: BattleState,
        llm_model_path: &str,
        embedding_model_path: &str,
        tactics_dir: &str,
    ) -> Self {
        let llm_agent = LlmGenerator::new(llm_model_path);
        
        let tactic_manager = match embedding::TacticManager::new(embedding_model_path, tactics_dir) {
            Ok(manager) => {
                println!("[System Initialization] 전술 임베딩 모델(CPU) 및 매니저가 로드되었습니다.");
                Some(manager)
            }
            Err(e) => {
                println!("[System Initialization] 전술 임베딩 모델 로드 실패: {}", e);
                None
            }
        };
        
        println!("==================================================");
        println!("[System Initialization] LLM 전술 분석 에이전트가 로드되었습니다.");
        println!("[Model Path] {}", llm_model_path);
        println!("[Embedding Path] {}", embedding_model_path);
        println!("==================================================");

        let (req_tx, req_rx) = crossbeam_channel::unbounded::<AsyncTacticRequest>();
        let (res_tx, res_rx) = crossbeam_channel::unbounded::<AsyncTacticResponse>();

        let llm_model_path_str = llm_model_path.to_string();
        let stop_required_worker = stop_required.clone();

        thread::Builder::new()
            .name("async_llm_worker".to_string())
            .spawn(move || {
                let worker_agent = LlmGenerator::new(&llm_model_path_str);
                println!("[LLM Worker] 백그라운드 비동기 추론 워커 스레드가 가동되었습니다.");
                while let Ok(req) = req_rx.recv() {
                    if stop_required_worker.load(Ordering::Relaxed) {
                        break;
                    }
                    let prompt = format!("Context:\n{}\nKeywords: {:?}\nCommand: {}\nNote: 입력된 순서대로 다중 전술(예: 이동 후 공격 등)이 있다면 then_order를 활용하여 순차적으로 실행되게 JSON을 구성하세요.", req.context_yaml, req.tactical_keywords, req.command);
                    let response_json = worker_agent.generate_tactics(&prompt);
                    if let Ok(json_str) = response_json {
                        let res = AsyncTacticResponse {
                            squad_uuid: req.squad_uuid,
                            json_str,
                            tactical_keywords: req.tactical_keywords,
                            command: req.command,
                        };
                        if let Err(e) = res_tx.send(res) {
                            println!("[LLM Worker] 결과 송신 채널 단절: {}", e);
                        }
                    }
                }
                println!("[LLM Worker] 백그라운드 워커 스레드가 종료되었습니다.");
            })
            .unwrap();

        Self {
            config,
            input,
            output,
            stop_required,
            last: Instant::now(),
            current_visibility: 0,
            battle_state: state,
            llm_agent,
            tactic_manager,
            logger: None,
            companies: std::collections::HashMap::new(),
            tactical_pings: std::collections::HashMap::new(),
            checkpoints: std::sync::RwLock::new(std::collections::HashMap::new()),
            scout_turn_offsets: std::sync::RwLock::new(std::collections::HashMap::new()),
            scouted_history: std::sync::RwLock::new(std::collections::HashMap::new()),
            async_llm_sender: req_tx,
            async_llm_receiver: res_rx,
        }
    }

    pub fn run(&mut self) -> Result<(), RunnerError> {
        loop {
            if self.stop_required.load(Ordering::Relaxed) {
                println!("Stopping runner ...");
                break;
            }

            let frame_i = self.battle_state.frame_i();
            puffin::profile_scope!("run", format!("frame {frame_i}"));
            puffin::GlobalProfiler::lock().new_frame();

            thread::sleep(self.sleep_duration());
            self.last = Instant::now();
            self.tick()?;
        }

        Ok(())
    }

    fn sleep_duration(&self) -> Duration {
        let elapsed = self.last.elapsed().as_micros() as u64;
        let target_duration = (self.config.target_cycle_duration_us as f32 / self.config.game_speed) as u64;
        if elapsed > target_duration {
            Duration::from_micros(0)
        } else {
            Duration::from_micros(target_duration - elapsed)
        }
    }
}

#[derive(Debug)]
pub enum RunnerError {
    InputChannelClosed,
    Output(SendError<Vec<OutputMessage>>),
}

impl From<SendError<Vec<OutputMessage>>> for RunnerError {
    fn from(error: SendError<Vec<OutputMessage>>) -> Self {
        Self::Output(error)
    }
}

impl Display for RunnerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunnerError::InputChannelClosed => f.write_str("Input channel closed"),
            RunnerError::Output(error) => f.write_str(&format!("Output error : {}", error)),
        }
    }
}
