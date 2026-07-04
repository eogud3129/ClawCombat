#!/bin/bash
# -*- coding: utf-8 -*-

# --- 환경 변수 설정 (macOS) ---
# LIBCLANG_PATH, PKG_CONFIG_PATH 등은 macOS에서 일반적으로 필요하지 않거나
# Homebrew 등을 통해 설치된 경로로 자동 설정됩니다.
# 필요에 따라 아래 주석을 해제하고 경로를 수정하세요.
# export LIBCLANG_PATH=/usr/local/opt/llvm/lib
# export PKG_CONFIG_PATH=/usr/local/lib/pkgconfig
# export PKG_CONFIG=/usr/local/bin/pkg-config
# export PATH=/usr/local/bin:$PATH

export RUST_BACKTRACE=1

echo "[Auto Build] 독립 워크스페이스 LLM 엔진(bonsai-pot) 자동 빌드 검사 중..."

# cargo build 명령어는 프로젝트 루트 디렉토리에서 실행되어야 합니다.
# llm_agent/crates/Cargo.toml 매니페스트 파일이 존재하는지 확인합니다.
cargo build --manifest-path llm_agent/crates/Cargo.toml --release --bin bonsai-pot --features="bench-internals"

# cargo build 결과 확인
if [ $? -ne 0 ]; then
    echo "[Error] LLM 엔진 자동 빌드에 실패했습니다. 프로세스를 중단합니다."
    exit 1
fi

echo "[Auto Build] 엔진 검증 완료. 메인 게임 클라이언트를 가동합니다..."

# macOS에서 실행할 바이너리 파일 지정 (battle_gui)
# --release 플래그로 빌드된 바이너리를 사용합니다.
cargo run --release --bin battle_gui -- Demo2 assets/demo2_deployment.json --embedded-server --server-rep-address=tcp://127.0.0.1:4255 --server-bind-address=tcp://127.0.0.1:4256 --side=a --side-a-control=W --side-a-control=NW --side-a-control=SW --side-b-control=ALL --init-sync
