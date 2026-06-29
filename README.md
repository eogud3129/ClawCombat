> ℹ️ **Notice:** This project is a [fork / modified version / extended version] of the original [OpenCombat](https://github.com/buxx/opencombat) repository developed by [buxx](https://github.com/buxx). All credits for the foundational codebase and core concepts go to the original author.


# ClawCombat

## Development

### Requirements

To be able to compile, please install (Debian packages example)

    build-essential cmake pkg-config libasound2-dev libfontconfig-dev libudev-dev libzmq3-dev

### Run

Add `--release` after `--bin battle_server` or after `--bin battle_gui` to disable debug and have normal performances.

#### Standalone server

    cargo run --bin battle_server --release -- Demo1 --rep-address tcp://127.0.0.1:4255 --bind-address tcp://127.0.0.1:4256

#### Standalone gui

Server must already been started

    cargo run --bin battle_gui --release -- Demo1 assets/demo1_deployment.json --server-rep-address tcp://127.0.0.1:4255 --server-bind-address tcp://127.0.0.1:4256 --side a --side-a-control N --side-a-control NW --side-a-control W --side-b-control ALL

#### Gui with embedded server

    cargo run --bin battle_gui --release -- Demo1 assets/demo1_deployment.json --embedded-server --server-rep-address tcp://127.0.0.1:4255 --server-bind-address tcp://127.0.0.1:4256 --side a --side-a-control N --side-a-control NW --side-a-control W --side-b-control ALL

-----

## mecab-ko-dict 설치방법

### 1. Rust 전용 사전 빌더 설치
cargo install mecab-ko-dict-builder

###  2. 은전한닢 사전 '원본 소스(CSV 등)' 다운로드
curl.exe -LO https://bitbucket.org/eunjeon/mecab-ko-dic/downloads/mecab-ko-dic-2.1.1-20180720.tar.gz

###  3. 압축 해제
tar.exe -xzf mecab-ko-dic-2.1.1-20180720.tar.gz

###  4. Rust 전용 바이너리 사전으로 변환 빌드! (model 폴더 하위에 mecab-ko-dic-rust 폴더로 생성)
mecab-ko-dict-builder build --input mecab-ko-dic-2.1.1-20180720 --output ./model/mecab-ko-dic-rust

-----

### Roadmap

- 옵시디언 그래프 전환을 위한 로그 최적화
- Yolo 모드에서 A,B side의 실내, 실외 Auto Pilot 전술 최적화
- 프롬프트로 전술 적용
- 전술 커스텀 매뉴얼(.md) 준비
- 프롬프트 입력시 자동완성으로 각 분대별 상황에 적합한 커스텀 매뉴얼(.md) 불러오기
- 다국어 NLP 가능한 모델 찾고, 싱글 페어 반영

-----

# Reference License
This project integrates and extends the following open-source works:

- Game Project [buxx/opencombat](https://github.com/buxx/opencombat) - AGPL-3.0
- Inference Engine [ruihe774/bonsai-pot](https://github.com/ruihe774/bonsai-pot) - Unlicense
- Inference Model [prism-ml/Bonsai-4B](https://huggingface.co/prism-ml/Bonsai-4B-gguf) - Apache-2.0
- Embedding Model [kekeappa/kor-static-embedding-512](https://huggingface.co/kekeappa/kor-static-embedding-512) - Apache-2.0
- NLP Engine [hephaex/mecab-ko](https://github.com/hephaex/mecab-ko) - Apache-2.0, MIT licenses
- NLP Model [eunjeon/mecab-ko-dic](https://bitbucket.org/eunjeon/mecab-ko-dic/downloads/) - Apache-2.0