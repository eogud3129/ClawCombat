> ℹ️ **Notice:** This project is a [fork / modified version / extended version] of the original [OpenCombat](https://github.com/buxx/opencombat) repository developed by [buxx](https://github.com/buxx). All credits for the foundational codebase and core concepts go to the original author.


# ClawCombat

## Development

### Requirements

To be able to compile, please install (Debian packages example)

    build-essential cmake pkg-config libasound2-dev libfontconfig-dev libudev-dev libzmq3-dev

### Run

Add `--release` after `--bin battle_server` or after `--bin battle_gui` to disable debug and have normal performances.

#### Standalone server

    cargo run --bin battle_server --release -- Demo1 --rep-address tcp://0.0.0.0:4255 --bind-address tcp://0.0.0.0:4256

#### Standalone gui

Server must already been started

    cargo run --bin battle_gui --release -- Demo1 assets/demo1_deployment.json --server-rep-address tcp://0.0.0.0:4255 --server-bind-address tcp://0.0.0.0:4256 --side a --side-a-control N --side-a-control NW --side-a-control W --side-b-control ALL

#### Gui with embedded server

    cargo run --bin battle_gui --release -- Demo1 assets/demo1_deployment.json --embedded-server --server-rep-address tcp://0.0.0.0:4255 --server-bind-address tcp://0.0.0.0:4256 --side a --side-a-control N --side-a-control NW --side-a-control W --side-b-control ALL



# Reference License
This project integrates and extends the following open-source works:

- Game Project [buxx/opencombat](https://github.com/buxx/opencombat) - AGPL-3.0
- Inference Engine [ruihe774/bonsai-pot](https://github.com/ruihe774/bonsai-pot) - Unlicense
- Inference Model [prism-ml/Bonsai-4B](https://huggingface.co/prism-ml/Bonsai-4B-gguf) - Apache-2.0
- Embedding Model [kekeappa/kor-static-embedding-512](https://huggingface.co/kekeappa/kor-static-embedding-512) - Apache-2.0