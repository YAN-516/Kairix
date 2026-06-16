#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${OUT_DIR:-$ROOT_DIR/target/netbench}"
ARCH="${ARCH:-riscv64}"
LOG_LEVEL="${LOG:-OFF}"
CPU="${CPU:-1}"
NET_BACKEND="${NET_BACKEND:-user}"
AUTO_TEST="${AUTO_TEST:-1}"
TIMEOUT_SEC="${TIMEOUT_SEC:-240}"
MAKE_TARGET="${MAKE_TARGET:-rkernel}"
BOOT_DELAY_SEC="${BOOT_DELAY_SEC:-8}"
TEST_COMMANDS="${TEST_COMMANDS:-cd /musl; sh netperf_testcode.sh; sh iperf_testcode.sh}"
DONE_PATTERN="${DONE_PATTERN:-#### OS COMP TEST GROUP END iperf}"
PARSE_LOG=""

usage() {
    cat <<'EOF'
Usage:
  tools/netbench-onekey.sh [options]

Run Kairix under QEMU, capture serial output, and collect final netperf/iperf
results into Markdown. Temporary logs and CSV are removed after parsing.

Options:
  --arch ARCH             riscv64 or loongarch64 (default: riscv64)
  --log-level LEVEL       Kernel LOG value passed to make (default: OFF)
  --cpu N                 QEMU CPU count (default: 1)
  --net-backend MODE      user, bridge, or auto (default: user)
  --auto-test VALUE       AUTO_TEST value passed to make (default: 1)
  --make-target TARGET    Top-level make target (default: rkernel)
  --boot-delay SEC        Delay before sending shell commands (default: 8)
  --commands COMMANDS     Shell commands sent to the guest after boot
  --done-pattern TEXT     Stop QEMU after this text appears in the log
  --timeout SEC           Stop QEMU after SEC seconds and parse partial log
  --out-dir DIR           Output directory (default: target/netbench)
  --parse-log FILE        Do not run QEMU; only parse an existing log
  -h, --help              Show this help

Environment variables with the same names can also be used:
  ARCH LOG CPU NET_BACKEND AUTO_TEST MAKE_TARGET BOOT_DELAY_SEC TEST_COMMANDS
  DONE_PATTERN TIMEOUT_SEC OUT_DIR
EOF
}

while (($#)); do
    case "$1" in
        --arch)
            ARCH="${2:?missing value for --arch}"
            shift 2
            ;;
        --log-level)
            LOG_LEVEL="${2:?missing value for --log-level}"
            shift 2
            ;;
        --cpu)
            CPU="${2:?missing value for --cpu}"
            shift 2
            ;;
        --net-backend)
            NET_BACKEND="${2:?missing value for --net-backend}"
            shift 2
            ;;
        --auto-test)
            AUTO_TEST="${2:?missing value for --auto-test}"
            shift 2
            ;;
        --make-target)
            MAKE_TARGET="${2:?missing value for --make-target}"
            shift 2
            ;;
        --boot-delay)
            BOOT_DELAY_SEC="${2:?missing value for --boot-delay}"
            shift 2
            ;;
        --commands)
            TEST_COMMANDS="${2:?missing value for --commands}"
            shift 2
            ;;
        --done-pattern)
            DONE_PATTERN="${2:?missing value for --done-pattern}"
            shift 2
            ;;
        --timeout)
            TIMEOUT_SEC="${2:?missing value for --timeout}"
            shift 2
            ;;
        --out-dir)
            OUT_DIR="${2:?missing value for --out-dir}"
            shift 2
            ;;
        --parse-log)
            PARSE_LOG="${2:?missing value for --parse-log}"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "unknown option: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

mkdir -p "$OUT_DIR"
RUN_ID="$(date +%Y%m%d-%H%M%S)"

if [[ -n "$PARSE_LOG" ]]; then
    RAW_LOG="$PARSE_LOG"
    BASENAME="$(basename "$PARSE_LOG")"
    CLEAN_LOG="$OUT_DIR/${BASENAME}.clean"
    CSV_OUT="$OUT_DIR/${BASENAME}.csv"
    MD_OUT="$OUT_DIR/${BASENAME}.md"
else
    RAW_LOG="$OUT_DIR/netbench-${ARCH}-${RUN_ID}.log"
    CLEAN_LOG="$OUT_DIR/netbench-${ARCH}-${RUN_ID}.clean.log"
    CSV_OUT="$OUT_DIR/netbench-${ARCH}-${RUN_ID}.csv"
    MD_OUT="$OUT_DIR/netbench-${ARCH}-${RUN_ID}.md"
fi

strip_log() {
    local input="$1"
    local output="$2"
    local esc
    esc="$(printf '\033')"
    tr -d '\000' < "$input" \
        | sed -E "s/${esc}\\[[0-9;?]*[ -/]*[@-~]//g" \
        | tr -d '\r' > "$output"
}

parse_results() {
    local input="$1"
    local csv="$2"
    local md="$3"

    awk -v csv="$csv" -v md="$md" '
        BEGIN {
            print "group,suite,test,status,metric,unit,role,raw" > csv
            print "| group | suite | test | status | metric | unit | role |" > md
            print "|---|---|---|---|---:|---|---|" > md
            printf "%-18s %-8s %-14s %-8s %-14s %-14s %s\n", "GROUP", "SUITE", "TEST", "STATUS", "METRIC", "UNIT", "ROLE"
            printf "%-18s %-8s %-14s %-8s %-14s %-14s %s\n", "-----", "-----", "----", "------", "------", "----", "----"
        }

        function trim(s) {
            gsub(/^[ \t]+|[ \t]+$/, "", s)
            return s
        }

        function csvq(s) {
            gsub(/"/, "\"\"", s)
            return "\"" s "\""
        }

        function reset_case() {
            metric = ""
            unit = ""
            role = ""
            raw = ""
        }

        function record() {
            if (group == "") group = "unknown"
            if (suite == "" || test == "") return
            if (status == "") status = "unknown"
            printable_metric = metric == "" ? "-" : metric
            printable_unit = unit == "" ? "-" : unit
            printable_role = role == "" ? "-" : role
            print csvq(group) "," csvq(suite) "," csvq(test) "," csvq(status) "," csvq(metric) "," csvq(unit) "," csvq(role) "," csvq(raw) >> csv
            print "| " group " | " suite " | " test " | " status " | " printable_metric " | " printable_unit " | " printable_role " |" >> md
            printf "%-18s %-8s %-14s %-8s %-14s %-14s %s\n", group, suite, test, status, printable_metric, printable_unit, printable_role
        }

        function parse_iperf_line(line,    n, t, i, value, rate_unit, found_role) {
            if (line !~ /bits\/sec/) return
            n = split(line, t, /[ \t]+/)
            found_role = ""
            for (i = 1; i <= n; i++) {
                if (t[i] == "sender" || t[i] == "receiver") found_role = t[i]
            }
            for (i = 2; i <= n; i++) {
                if (t[i] ~ /bits\/sec/) {
                    value = t[i - 1]
                    rate_unit = t[i]
                    break
                }
            }
            if (value == "") return
            if (metric == "" || found_role == "receiver" || role != "receiver") {
                metric = value
                unit = rate_unit
                role = found_role
                raw = line
            }
        }

        function parse_netperf_line(line,    tmp, n, t, i, numeric_count, last_numeric) {
            tmp = trim(line)
            if (tmp == "") return
            gsub(/[0-9.+-]/, "", tmp)
            gsub(/[ \t]/, "", tmp)
            if (tmp != "") return

            n = split(line, t, /[ \t]+/)
            numeric_count = 0
            last_numeric = ""
            for (i = 1; i <= n; i++) {
                if (t[i] ~ /^[+-]?[0-9]+(\.[0-9]+)?$/) {
                    numeric_count++
                    last_numeric = t[i]
                }
            }
            if (numeric_count < 4 || last_numeric == "") return

            metric = last_numeric
            if (test ~ /STREAM/) {
                unit = "10^6bits/s"
            } else if (test ~ /RR|CRR/) {
                unit = "trans/s"
            } else {
                unit = "netperf"
            }
            role = ""
            raw = line
        }

        /^#### OS COMP TEST GROUP START / {
            group = $0
            sub(/^#### OS COMP TEST GROUP START /, "", group)
            sub(/ ####$/, "", group)
            next
        }

        /^====== (netperf|iperf) .* begin ======/ {
            line = $0
            sub(/^====== /, "", line)
            sub(/ begin ======$/, "", line)
            split(line, parts, /[ \t]+/)
            suite = parts[1]
            test = parts[2]
            status = "running"
            in_case = 1
            reset_case()
            next
        }

        /^====== (netperf|iperf) .* end: / {
            line = $0
            sub(/^====== /, "", line)
            sub(/ ======$/, "", line)
            split(line, parts, /[ \t]+/)
            suite = parts[1]
            test = parts[2]
            status = parts[4]
            in_case = 0
            record()
            reset_case()
            next
        }

        in_case && suite == "iperf" {
            parse_iperf_line($0)
            next
        }

        in_case && suite == "netperf" {
            parse_netperf_line($0)
            next
        }

        END {
            if (in_case) {
                status = "incomplete"
                record()
            }
        }
    ' "$input"
}

RUNNER_PID=""
INPUT_PID=""
INPUT_FIFO=""

stop_runner() {
    if [[ -n "$INPUT_PID" ]]; then
        kill "$INPUT_PID" 2>/dev/null || true
    fi
    if [[ -n "$RUNNER_PID" ]] && kill -0 "$RUNNER_PID" 2>/dev/null; then
        kill -TERM "-$RUNNER_PID" 2>/dev/null || kill -TERM "$RUNNER_PID" 2>/dev/null || true
        sleep 1
        if kill -0 "$RUNNER_PID" 2>/dev/null; then
            kill -KILL "-$RUNNER_PID" 2>/dev/null || kill -KILL "$RUNNER_PID" 2>/dev/null || true
        fi
    fi
}

cleanup_input() {
    if [[ -n "$INPUT_FIFO" ]]; then
        rm -f "$INPUT_FIFO"
    fi
}

if [[ -z "$PARSE_LOG" ]]; then
    echo "[netbench] running: make $MAKE_TARGET LOG=$LOG_LEVEL"
    echo "[netbench] raw log: $RAW_LOG"
    echo "[netbench] guest commands after ${BOOT_DELAY_SEC}s: $TEST_COMMANDS"
    echo "[netbench] stop pattern: $DONE_PATTERN"
    touch "$RAW_LOG"
    INPUT_FIFO="$(mktemp -u "$OUT_DIR/netbench-input.XXXXXX")"
    mkfifo "$INPUT_FIFO"
    trap 'echo "[netbench] interrupted; stopping QEMU and parsing partial log"; stop_runner' INT TERM
    set +e

    setsid bash -c 'cd "$1" && exec timeout "$2" make "$3" "LOG=$4"' \
        _ "$ROOT_DIR" "$TIMEOUT_SEC" "$MAKE_TARGET" "$LOG_LEVEL" \
        < "$INPUT_FIFO" > >(tee "$RAW_LOG") 2>&1 &
    RUNNER_PID=$!

    {
        sleep "$BOOT_DELAY_SEC"
        printf '%s\n' "$TEST_COMMANDS"
    } > "$INPUT_FIFO" &
    INPUT_PID=$!

    run_status=0
    while kill -0 "$RUNNER_PID" 2>/dev/null; do
        if grep -Fq "$DONE_PATTERN" "$RAW_LOG"; then
            echo "[netbench] found stop pattern; stopping QEMU"
            stop_runner
            break
        fi
        sleep 1
    done

    wait "$RUNNER_PID"
    run_status=$?
    wait "$INPUT_PID" 2>/dev/null || true
    sleep 1
    cleanup_input
    trap - INT TERM
    set -e
    if [[ "$run_status" -eq 124 ]]; then
        echo "[netbench] QEMU timed out after ${TIMEOUT_SEC}s; parsing partial log"
    elif [[ "$run_status" -ne 0 ]]; then
        echo "[netbench] QEMU/make exited with status $run_status; parsing available log"
    fi
fi

strip_log "$RAW_LOG" "$CLEAN_LOG"

echo
echo "[netbench] final result summary"
parse_results "$CLEAN_LOG" "$CSV_OUT" "$MD_OUT"

rm -f "$CLEAN_LOG" "$CSV_OUT"
if [[ -z "$PARSE_LOG" ]]; then
    rm -f "$RAW_LOG"
fi

echo
echo "[netbench] Markdown:    $MD_OUT"
