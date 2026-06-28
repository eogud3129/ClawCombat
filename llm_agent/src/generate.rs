use crate::model::LlmAgent;
use anyhow::Result;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use tokenizers::models::bpe::{BPE, Vocab};
use tokenizers::pre_tokenizers::byte_level::ByteLevel;
use tokenizers::pre_tokenizers::sequence::Sequence;
use tokenizers::pre_tokenizers::split::{Split, SplitPattern};
use tokenizers::pre_tokenizers::PreTokenizerWrapper;
use tokenizers::{AddedToken, SplitDelimiterBehavior, Tokenizer};

const QWEN2_RE: &str = concat!(
    r"(?i:'s|'t|'re|'ve|'m|'ll|'d)|",
    r"[^\r\n\p{L}\p{N}]?\p{L}+|",
    r"\p{N}|",
    r" ?[^\s\p{L}\p{N}]+[\r\n]*|",
    r"\s*[\r\n]+|",
    r"\s+(?!\S)|",
    r"\s+",
);

pub struct LlmGenerator {
    pub agent: LlmAgent,
    model_dir: String,
    tokenizer: Tokenizer,
}

impl LlmGenerator {
    pub fn new(model_dir: &str) -> Self {
        let agent = LlmAgent::new(model_dir).expect("Failed to initialize LlmAgent");
        // 파이썬 스크립트 의존성을 제거하기 위해 모델 디렉토리에서 보카 트리를 직접 로드
        let model_path = Path::new(model_dir);
        let tokenizer = Self::build_native_tokenizer(model_path);

        Self { 
            agent,
            model_dir: model_dir.to_string(),
            tokenizer,
        }
    }

    fn build_native_tokenizer(model_dir: &Path) -> Tokenizer {
        let vocab_bytes = fs::read(model_dir.join("vocab.bin")).expect("vocab.bin not found");
        let vocab_offsets_bytes = fs::read(model_dir.join("vocab_offsets.bin")).expect("vocab_offsets.bin not found");
        
        let offs: &[u32] = bytemuck::cast_slice(&vocab_offsets_bytes);
        let n_vocab = offs.len() - 1;

        let mut token_strs = Vec::with_capacity(n_vocab);
        for i in 0..n_vocab {
            let s = std::str::from_utf8(&vocab_bytes[offs[i] as usize..offs[i + 1] as usize])
                .unwrap_or("?")
                .to_string();
            token_strs.push(s);
        }

        let specials: Vec<AddedToken> = token_strs
            .iter()
            .filter(|s| s.starts_with("<|") && s.ends_with("|>"))
            .map(|s| AddedToken::from(s.as_str(), true))
            .collect();

        let vocab: Vocab = token_strs
            .into_iter()
            .enumerate()
            .map(|(i, s)| (s, i as u32))
            .collect();

        let merges: Vec<(String, String)> = fs::read_to_string(model_dir.join("merges.txt"))
            .expect("merges.txt not found")
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| {
                let (a, b) = l.split_once(' ').expect("malformed merges.txt");
                (a.to_owned(), b.to_owned())
            })
            .collect();

        let bpe = BPE::builder()
            .vocab_and_merges(vocab, merges)
            .byte_fallback(false)
            .build()
            .expect("build BPE model");

        let mut tok = Tokenizer::new(bpe);

        let split = Split::new(
            SplitPattern::Regex(QWEN2_RE.to_owned()),
            SplitDelimiterBehavior::Isolated,
            true,
        )
        .expect("build Split pretokenizer");
        
        tok.with_pre_tokenizer(Some(PreTokenizerWrapper::Sequence(Sequence::new(vec![
            PreTokenizerWrapper::Split(split),
            PreTokenizerWrapper::ByteLevel(ByteLevel::new(false, false, false)),
        ]))));

        let _ = tok.add_special_tokens(specials);
        tok
    }

    pub fn generate_tactics(&self, prompt: &str) -> Result<String> {
        // 하드웨어 버퍼 1024 토큰 상한을 초과하여 generate 크래시가 발생하는 것을 방지하기 위해, 
        // 출력 포맷을 정밀하고 간결한 구조화 JSON으로 제한하도록 시스템 지침을 명확히 압축 주입합니다.
        let segment = format!(
            "<|im_start|>system\nYou are a military tactician. Respond ONLY with a compact JSON object fitting the PRD specification without markdown code blocks or extra natural language explanations.<|im_end|>\n<|im_start|>user\n{}<|im_end|>\n<|im_start|>assistant\n",
            prompt
        );

        // Rust 내부 토크나이저를 사용해 문자열을 즉시 u32 토큰 벡터 배열로 인코딩
        let encoded = self.tokenizer.encode(segment.as_str(), false)
            .expect("Failed to encode prompt natively");
        let tokens: &[u32] = encoded.get_ids();

        // 컴파일된 엔진 바이너리 프로세스를 직접 열고 입출력 파이프라인 결합
        // 생성 토큰 상한(--max-new-tokens)을 512에서 128로 하향 조정하여, 인풋 컨텍스트 자원과 합산했을 때 1024 슬롯을 절대 침범하지 않도록 제어합니다.
        let mut bonsai_process = Command::new("llm_agent/crates/target/release/bonsai-pot.exe")
            .args(&[
                &self.model_dir, 
                "--max-new-tokens", "128", 
                "--mode", "prompt",
                "--temperature", "0.0",
                "--top-p", "1.0",
                "--top-k", "80"
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        // 백그라운드 프로세스의 stdin에 u32 토큰의 원시 바이트 배열 스트림을 직접 주입
        {
            let mut stdin = bonsai_process.stdin.take().expect("Failed to open engine stdin");
            let token_bytes: &[u8] = bytemuck::cast_slice(tokens);
            stdin.write_all(token_bytes)?;
            stdin.flush()?;
        }

        // 프로세스 종료 및 결과 데이터 동기 회수
        let output = bonsai_process.wait_with_output()?;
        let raw_response = String::from_utf8_lossy(&output.stdout).to_string();
        let error_response = String::from_utf8_lossy(&output.stderr).to_string();
        
        let response = if !output.status.success() || raw_response.is_empty() {
            format!(
                "엔진 실행 실패 (Exit Code: {:?})\n--- Stderr 에러 로그 ---\n{}",
                output.status.code(),
                error_response
            )
        } else if let Some(idx) = raw_response.find("<|im_start|>assistant\n") {
            raw_response[idx + "<|im_start|>assistant\n".len()..].to_string()
        } else {
            raw_response
        };
        
        Ok(response)
    }
}