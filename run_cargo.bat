@echo off
chcp 65001 > nul
call "C:\Program Files (x86)\Microsoft Visual Studio\2019\BuildTools\VC\Auxiliary\Build\vcvars64.bat"

:: LLVM Clang 경로 설정 (bindgen 에러 해결)
set LIBCLANG_PATH=C:\Program Files\LLVM\bin

:: 1. vcpkg가 설치한 실제 libzmq의 설정 파일 경로 지정
set PKG_CONFIG_PATH=C:\vcpkg\installed\x64-windows\lib\pkgconfig

:: 2. vcpkg가 설치한 실제 pkgconf 도구의 경로 지정 (올바른 경로)
set PKG_CONFIG=C:\vcpkg\installed\x64-windows\tools\pkgconf\pkgconf.exe

:: DLL을 찾을 수 있도록 PATH에 vcpkg bin 폴더 추가 (STATUS_DLL_NOT_FOUND 해결)
set PATH=C:\vcpkg\installed\x64-windows\bin;%PATH%

:: 3. 전체 프로젝트 사전 빌드 (oc_launcher가 컴파일된 battle_gui.exe를 찾을 수 있도록 함)
cargo build --release

:: 4. 서버 실행 (새 CMD 창을 띄워서 백그라운드로 실행)
start "OpenCombat Server" cargo run --bin battle_server --release -- Demo1 --rep-address tcp://0.0.0.0:4255 --bind-address tcp://0.0.0.0:4256

:: 5. 서버가 구동될 때까지 2초 대기
timeout /t 2 /nobreak >nul

:: 6. UI 런처 실행 (서버 구동 후 클라이언트 실행)
cargo run --bin oc_launcher --release