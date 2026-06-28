---
id: sneak_flank
name: 은밀 우회 기동
priority: 2
counters: ["suppress_fire"] # 적이 제압 사격으로 화력을 쏟아부을 때 사선을 피함
trigger_conditions:
  surrounding_terrain: ["HighGrass", "Underbrush"]
---

### 은밀 우회 기동
적의 화망을 피해 포복 상태로 지형지물을 우회하여 접근합니다.

* **1차 행동**: `Order::SneakTo` 상태로 기동.
* **리스크**: 기동 속도가 매우 느려 목표 도달 시간이 지연됩니다.