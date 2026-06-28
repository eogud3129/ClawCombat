---
id: suppress_fire
name: 즉각 대응 사격 (화력 우선)
priority: 1
counters: ["move_fast_openfield"] # 적이 개활지 강행 돌파 중일 때 무력화 가능
trigger_conditions:
  enemy_visibility: "Detected"
  feeling_under_fire: ["Normal", "Warning"]
---

### 즉각 대응 사격 (화력 우선)
현재 위치에서 기동을 멈추고 가장 가까운 적 분대에게 화력을 집중하여 제압합니다.

* **1차 행동**: 이동 명령을 즉각 취소하고 `Order::SuppressFire` 실행.
* **리스크**: 탄약 소모가 극심하며 후방 노출 위험이 있습니다.