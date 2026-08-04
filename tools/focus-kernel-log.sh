#!/usr/bin/env bash
# Keep the full kernel log unchanged while presenting a compact diagnostic view.

set -o errexit
set -o nounset
set -o pipefail

usage() {
    printf '%s\n' \
        'Usage: focus-kernel-log.sh [--all-focus] [LOG_FILE]' \
        '' \
        'Read LOG_FILE, or stdin when LOG_FILE is omitted or is "-".' \
        'The default view keeps fatal errors and workload snapshots, samples' \
        'repeated stall/slow-path records, and hides per-call trace noise.' \
        '' \
        'Options:' \
        '  --all-focus  Do not sample records that belong to the focused view.' \
        '  -h, --help   Show this help.'
}

sample_repeats=1
input=-

while (($# > 0)); do
    case "$1" in
        --all-focus)
            sample_repeats=0
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        -)
            input=-
            ;;
        --*)
            printf 'focus-kernel-log.sh: unknown option: %s\n' "$1" >&2
            usage >&2
            exit 2
            ;;
        *)
            if [[ "$input" != - ]]; then
                printf 'focus-kernel-log.sh: only one input file is supported\n' >&2
                exit 2
            fi
            input=$1
            ;;
    esac
    shift
done

if [[ "$input" != - && ! -r "$input" ]]; then
    printf 'focus-kernel-log.sh: cannot read: %s\n' "$input" >&2
    exit 1
fi

LC_ALL=C awk -v sample_repeats="$sample_repeats" '
BEGIN {
    retained_records = 0
}

function emit(line) {
    print line
    fflush()
}

function emit_record(line) {
    ++retained_records
    emit(line)
}

function record_tag(line,    text) {
    text = line
    sub(/^.*\[(ERROR|WARN|INFO|DEBUG|TRACE)\][[:space:]]*/, "", text)
    if (match(text, /\[[A-Z][A-Z0-9_]+\]/)) {
        return substr(text, RSTART + 1, RLENGTH - 2)
    }
    return "UNTAGGED"
}

function is_power_of_two(value) {
    while (value > 1 && value % 2 == 0) {
        value /= 2
    }
    return value == 1
}

function emit_sampled(line,    tag, count) {
    tag = record_tag(line)
    count = ++seen[tag]
    if (!sample_repeats || count <= 4 || is_power_of_two(count)) {
        if (sample_repeats && count > 4) {
            emit("[FOCUS_SAMPLE] tag=" tag " occurrence=" count \
                 " suppressed_since_previous=" (count - last_emitted[tag] - 1))
        }
        emit_record(line)
        last_emitted[tag] = count
    } else {
        ++suppressed[tag]
    }
}

{
    line = $0

    # Preserve normal console/build output.  The compact view targets noisy
    # kernel ERROR diagnostics, and must not hide prompts or workload progress.
    if (line !~ /\[ERROR\]/) {
        emit_record(line)
        next
    }

    # Always retain fatal failures and evidence of data/metadata corruption.
    if (line ~ /\[(OOM|SIGNAL_FATAL|EXT4_WRITEBACK_EIO|LWEXT4_EIO|LWEXT4_BCACHE_BUFFER_IDENTITY_CORRUPTION|LWEXT4_FWRITE_SOURCE_CORRUPTION|LWEXT4_FWRITE_STATE_CORRUPTION)\]/ ||
        line ~ /\[TASK_EXIT_FATAL\].*exit_code=[1-9][0-9]*/ ||
        tolower(line) ~ /(kernel panic|(^|[^[:alnum:]_])panic(ed)?([^[:alnum:]_]|$)|(^|[^[:alnum:]_])fatal([^[:alnum:]_]|$)|assertion failed|out of memory)/) {
        emit_record(line)
        next
    }

    # The timeout workload snapshot is intentionally complete: its task rows
    # are needed together to distinguish useful work from scheduler/I/O stalls.
    if (line ~ /\[FUTEX_TIMEOUT_WORKLOAD\]/ ||
        (line ~ /\[TASK_RUNTIME_STALL(_SUMMARY)?\]/ &&
         line ~ /snapshot_tag=FUTEX_TIMEOUT_WORKLOAD/)) {
        emit_record(line)
        next
    }

    # These cumulative phase records are emitted next to workload snapshots.
    # Keep a bounded sample so the compact view exposes attribution without
    # retaining syscall- or fault-level detail lines.
    if (line ~ /\[(MPROTECT_PHASE_TOTALS|ANON_FAULT_PHASE_TOTALS|BLOCK_IO_COALESCE_TOTALS)\]/) {
        emit_sampled(line)
        next
    }

    # Keep lock, scheduler, trap, TLB, syscall, and I/O stall evidence, while
    # exponentially sampling a tag when the same detector fires repeatedly.
    if (line ~ /\[(CONTEXT_SWITCH_STALL_DETAIL|PROCESS_LOCK_STALL_DETAIL|SCHEDULER_CPU_STALLED(_VISIBLE)?|TIMER_IRQ_SCHED_STALL(_CONTEXT|_DETAIL|_VISIBLE)?|TIMER_RECOVERY_STALL_(CONTEXT|PROGRESS)|TLB_SHOOTDOWN_STALL_DETAIL|PAGE_FAULT_STALL_DETAIL|TRAP_STALL_DETAIL|SYSCALL_STALL(_VISIBLE)?|WORKLOAD_PROGRESS_STALL|EXECVE_STALL|LWEXT4_STAGE3_STALL|FUTEX_STALL_SNAPSHOT)\]/) {
        emit_sampled(line)
        next
    }

    # Retain slow-operation conclusions, but omit their high-volume per-stage
    # traces/details unless they become part of a stall snapshot above.
    if (line ~ /\[(LWEXT4_SLOW_OP|READLINKAT_PATH_SLOW|READLINKAT_STEP_SLOW|READLINKAT_LWEXT4_SLOW|MMAP_SHARED_WRITEBACK|VIRTIO_BLK_LWEXT4_FWRITE_DETAIL|ANON_FAULT_SLOW)\]/) {
        emit_sampled(line)
        next
    }

    # Count the most common known noise so the compact view still explains
    # what it omitted without repeating every syscall-level record.
    if (line ~ /\[(MPROTECT_TRACE|MPROTECT_DETAIL|MPROTECT_GAP_DETAIL|FUTEX_WAIT_QUEUED|FUTEX_WAIT_RESULT|FUTEX_WAKE_RESULT|FUTEX_WAKE_DELIVERY|READLINKAT_TRACE|READLINKAT_LWEXT4_FIND_DETAIL|READLINKAT_LWEXT4_FIND_GAP)\]/) {
        ++noise[record_tag(line)]
        next
    }
    if (line ~ /futex_wait:.*val=/) {
        ++noise["FUTEX_WAIT_LEGACY"]
        next
    }
}

END {
    if (!sample_repeats) {
        exit
    }
    for (tag in suppressed) {
        if (suppressed[tag] > 0) {
            emit("[FOCUS_SUMMARY] tag=" tag " seen=" seen[tag] \
                 " displayed=" (seen[tag] - suppressed[tag]) \
                 " suppressed=" suppressed[tag])
        }
    }
    for (tag in noise) {
        emit("[FOCUS_NOISE] tag=" tag " hidden=" noise[tag])
    }
    emit("[FOCUS_TOTAL] input_lines=" NR " retained_records=" retained_records \
         " hidden_lines=" (NR - retained_records))
}
' "$input"
