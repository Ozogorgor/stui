#!/bin/bash
SOCK=/tmp/stui-test.sock

# Step 1: Preview
echo ">>> Running preview..."
PREVIEW=$(printf '%s\n' '{"type":"action_a_tags_preview","id":"t2","scope":{"kind":"album","artist":"Aphex Twin","album":"Richard D. James Album","date":""}}' \
  | socat -t 5 - UNIX-CONNECT:$SOCK)

# Extract job_id from the preview response line
JOB_ID=$(echo "$PREVIEW" | grep 'action_a_tags_preview' | python3 -c "import sys,json; print(json.loads(sys.stdin.read())['job_id'])")

if [ -z "$JOB_ID" ]; then
  echo "ERROR: couldn't extract job_id from preview response:"
  echo "$PREVIEW"
  exit 1
fi

echo ">>> Preview returned job_id: $JOB_ID"
echo ">>> Preview rows:"
echo "$PREVIEW" | grep 'action_a_tags_preview' | python3 -c "import sys,json; [print(f'  {r[\"field\"]}: {r[\"old_value\"]} → {r[\"new_value\"]}') for r in json.loads(sys.stdin.read())['rows']]"

# Step 2: Apply
echo ""
echo ">>> Applying..."
APPLY=$(printf '%s\n' "{\"type\":\"action_a_tags_apply\",\"id\":\"t3\",\"job_id\":\"$JOB_ID\"}" \
  | socat -t 10 - UNIX-CONNECT:$SOCK)

echo ">>> Apply response:"
echo "$APPLY" | grep 'action_a_tags_apply'

# Step 3: Verify
echo ""
echo ">>> Checking sidecar backups..."
ls -la /home/ozogorgor/Music/richard-d.-james-album-flac/*.stui-tag-backup.json 2>/dev/null || echo "  (none found)"

echo ""
echo ">>> Checking updated tags..."
ffprobe "/home/ozogorgor/Music/richard-d.-james-album-flac/06. Aphex Twin - To Cure A Weakling Child.flac" 2>&1 | grep -i "title\|date" || echo "  (ffprobe failed)"
