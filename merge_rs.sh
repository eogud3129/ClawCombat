#!/usr/bin/env bash

# 결과가 저장될 파일명
OUTPUT_FILE="full.txt"

# 기존에 full.txt가 있다면 초기화 (덮어쓰기 위해)
> "$OUTPUT_FILE"

# 제외할 폴더들을 제외하고 .rs 파일만 찾아서 순회
find . -type d \( -name "target" -o -name "node_modules" -o -name ".git" -o -name "llm_agent" \) -prune -o -type f -name "*.rs" -print | while read -r file; do
    
    # 구분선 및 파일 경로 기록
    echo "=========================================" >> "$OUTPUT_FILE"
    # './' 로 시작하는 경로를 깔끔하게 보여주기 위해 가공 (선택 사항)
    echo "File: ${file#./}" >> "$OUTPUT_FILE"
    echo "-----------------------------------------" >> "$OUTPUT_FILE"
    
    # 파일 내용 추가
    cat "$file" >> "$OUTPUT_FILE"
    
    # 마지막에 빈 줄 하나 추가
    echo "" >> "$OUTPUT_FILE"

done

# 완료 메시지 녹색으로 출력 (ANSI 색상 코드 사용)
GREEN='\033[0;32m'
NC='\033[0m' # 색상 초기화
echo -e "${GREEN}완료!${NC}"