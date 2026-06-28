use anyhow::{Error as E, Result};
use candle_core::{Device, Tensor};
use tokenizers::Tokenizer;
use std::fs;
use std::path::Path;

pub struct EmbeddingModel {
    embeddings: Option<Tensor>, // 정적 임베딩 텐서. Shape: (vocab_size, hidden_size)
    tokenizer: Tokenizer,
    device: Device,
    hidden_size: usize,
}

impl EmbeddingModel {
    pub fn new<P: AsRef<Path>>(model_path: P) -> Result<Self> {
        let device = Device::Cpu;
        let path = model_path.as_ref();
        
        let tokenizer_path = path.join("tokenizer.json");
        let weights_path = path.join("model.safetensors");

        // 토크나이저 로드
        let tokenizer = Tokenizer::from_file(&tokenizer_path).map_err(|e| E::msg(e.to_string()))?;

        // 정적 임베딩 전용 로직: 복잡한 Config나 VarBuilder를 완전히 우회하고, 파일에서 텐서를 직접 강제 추출합니다.
        let embeddings = match candle_core::safetensors::load(&weights_path, &device) {
            Ok(mut tensors) => {
                // 가장 흔한 이름부터 확인 후, 없으면 파일 내에서 가장 큰 텐서를 임베딩 행렬로 간주합니다.
                let tensor = if let Some(t) = tensors.remove("embeddings.word_embeddings.weight") {
                    t
                } else if let Some(t) = tensors.remove("model.embeddings.word_embeddings.weight") {
                    t
                } else if let Some(t) = tensors.remove("weight") {
                    t
                } else {
                    tensors.into_values().max_by_key(|t| t.elem_count()).unwrap()
                };
                Some(tensor.to_dtype(candle_core::DType::F32)?)
            },
            Err(e) => {
                println!("[Embedding] 정적 임베딩 모델 로드 실패. 임시 비활성화합니다. 에러: {}", e);
                None
            }
        };

        // 행렬에서 두 번째 차원(열)을 추출하여 모델의 은닉층 크기(hidden_size)를 동적으로 알아냅니다.
        let hidden_size = embeddings.as_ref().map(|t| t.dim(1).unwrap_or(512)).unwrap_or(512);

        Ok(Self {
            embeddings,
            tokenizer,
            device,
            hidden_size,
        })
    }

    pub fn get_embedding(&self, text: &str) -> Result<Vec<f32>> {
        let Some(embeddings) = &self.embeddings else {
            // 모델 로드 실패 상태일 경우, 프로그램 크래시를 막기 위해 더미 벡터를 반환합니다.
            return Ok(vec![0.0; self.hidden_size]);
        };

        let tokens = self.tokenizer.encode(text, true).map_err(|e| E::msg(e.to_string()))?;
        let token_ids = tokens.get_ids();
        
        if token_ids.is_empty() {
            return Ok(vec![0.0; self.hidden_size]);
        }

        let token_ids_tensor = Tensor::new(token_ids, &self.device)?;
        
        // 단어 벡터 룩업 (Index Select): (seq_len, hidden_size) 텐서 생성
        let token_embeddings = embeddings.index_select(&token_ids_tensor, 0)?;
        
        // Mean Pooling: 문장 내 포함된 전체 토큰 벡터들의 평균을 계산하여 하나의 대표 문장 벡터로 압축합니다.
        let seq_len = token_ids.len() as f64;
        let sum_embedding = token_embeddings.sum(0)?;
        let mean_embedding = (sum_embedding / seq_len)?;
        
        let vec: Vec<f32> = mean_embedding.to_vec1()?;
        
        Ok(vec)
    }
}

pub struct TacticTemplate {
    pub id: String,
    pub name: String,
    pub embedding: Vec<f32>,
}

pub struct TacticManager {
    model: EmbeddingModel,
    templates: Vec<TacticTemplate>,
}

impl TacticManager {
    pub fn new(model_path: &str, tactics_dir: &str) -> Result<Self> {
        let model = EmbeddingModel::new(model_path)?;
        let mut templates = Vec::new();

        if let Ok(entries) = fs::read_dir(tactics_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("md") {
                    if let Ok(content) = fs::read_to_string(&path) {
                        let id = Self::extract_yaml_field(&content, "id:").unwrap_or_else(|| path.file_stem().unwrap().to_string_lossy().to_string());
                        let name = Self::extract_yaml_field(&content, "name:").unwrap_or_else(|| id.clone());

                        // TODO: 임베딩 텍스트 품질 향상을 위해 내용(Content) 요약 등 결합 고려. 현재는 name 기반으로 벡터화 진행
                        if let Ok(embedding) = model.get_embedding(&name) {
                            templates.push(TacticTemplate { id, name, embedding });
                        }
                    }
                }
            }
        }
        
        Ok(Self { model, templates })
    }

    pub fn search(&self, query: &str, top_k: usize) -> Vec<(String, String, f32)> {
        if query.trim().is_empty() { return vec![]; }
        
        let query_emb = match self.model.get_embedding(query) {
            Ok(emb) => emb,
            Err(_) => return vec![],
        };

        let mut results: Vec<_> = self.templates.iter().map(|t| {
            let sim = Self::cosine_similarity(&query_emb, &t.embedding);
            (t.id.clone(), t.name.clone(), sim)
        }).collect();

        // 유사도 기준 내림차순 정렬
        results.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        results.into_iter().take(top_k).collect()
    }

    fn extract_yaml_field(content: &str, field: &str) -> Option<String> {
        content.lines()
            .find(|line| line.trim().starts_with(field))
            .map(|line| line.trim().trim_start_matches(field).trim().trim_matches('"').to_string())
    }

    fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm_a == 0.0 || norm_b == 0.0 { 0.0 } else { dot / (norm_a * norm_b) }
    }
}