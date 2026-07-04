@echo off
chcp 65001 > nul

set LIBCLANG_PATH=C:\Program Files\LLVM\bin
set PKG_CONFIG_PATH=C:\vcpkg\installed\x64-windows\lib\pkgconfig
set PKG_CONFIG=C:\vcpkg\installed\x64-windows\tools\pkgconf\pkgconf.exe
set PATH=C:\Windows\System32;C:\vcpkg\installed\x64-windows\bin;%PATH%
set RUST_BACKTRACE=1

echo [Auto Build] 독립 워크스페이스 LLM 엔진(bonsai-pot) 자동 빌드 검사 중...
cargo build --manifest-path llm_agent/crates/Cargo.toml --release --bin bonsai-pot --features="bench-internals"
if errorlevel 1 (
    echo [Error] LLM 엔진 자동 빌드에 실패했습니다. 프로세스를 중단합니다.
    exit /b 1
)

echo [Auto Build] 엔진 검증 완료. 메인 게임 클라이언트를 가동합니다...
cargo run --release --bin battle_gui -- Demo2 assets/demo2_deployment.json --embedded-server --server-rep-address=tcp://127.0.0.1:4255 --server-bind-address=tcp://127.0.0.1:4256 --side=a --side-a-control=W --side-a-control=NW --side-a-control=SW --side-b-control=ALL --init-sync	
