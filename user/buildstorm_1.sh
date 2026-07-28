#!/bin/sh
# BuildStorm testcode (contestant-facing reference) -- runs INSIDE the guest,
# on the kernel under test. The guest image is a self-contained rootfs
# (Debian glibc + rust toolchain + tgoskits sources + cargo cache); the student
# mounts it as their rootfs.
#
# Output (parsed by judge/judge_buildstorm.py):
#   BUILDSTORM_TOOLCHAIN ok|fail                         (8 points)
#   BUILDSTORM_MINIBUILD ok|fail                         (12 points)
#   BUILDSTORM_COMPILE mode=multi ok=true|false elapsed_s=<s> cores=<n> bytes=<n> arch=<a>
#                                                         (40 + 120 points)

echo "#### OS COMP TEST GROUP START buildstorm ####"
echo "BUILDSTORM_DIAG_VERSION 2026-07-28.8"

mount -t proc proc /proc 2>/dev/null
mount -t sysfs sysfs /sys 2>/dev/null
mount -t devtmpfs devtmpfs /dev 2>/dev/null

export PATH=/root/.cargo/bin:/usr/local/bin:/usr/bin:/bin:/sbin:/usr/sbin
export HOME=/root RUSTUP_HOME=/root/.rustup CARGO_HOME=/root/.cargo
export RUSTUP_TOOLCHAIN=nightly-2026-05-28
export CARGO_NET_OFFLINE=true

# Keep diagnostics coarse enough not to perturb the build, while still making
# a stalled Cargo status line distinguishable from useful rustc work, page-cache
# churn, or a parent waiting for children that have already exited.
BUILDSTORM_DIAG_INTERVAL=30
BUILDSTORM_ACTIVE_PID=
BUILDSTORM_MONITOR_PID=
BUILDSTORM_DIAG_LOG=
BUILDSTORM_REPLAY_TMPFS=/tmp/buildstorm-replay-tmpfs
BUILDSTORM_REPLAY_TMPFS_MOUNTED=false

mkdir -p "$BUILDSTORM_REPLAY_TMPFS" 2>/dev/null
if mount -t tmpfs -o size=1g tmpfs "$BUILDSTORM_REPLAY_TMPFS" 2>/dev/null; then
    BUILDSTORM_REPLAY_TMPFS_MOUNTED=true
    echo "BUILDSTORM_DIAG_REPLAY_TMPFS available=true path=$BUILDSTORM_REPLAY_TMPFS"
else
    echo "BUILDSTORM_DIAG_REPLAY_TMPFS available=false path=$BUILDSTORM_REPLAY_TMPFS"
fi

buildstorm_diag_snapshot() {
    DIAG_PHASE=$1
    DIAG_EVENT=$2
    DIAG_TARGET_PID=$3
    DIAG_UPTIME=$(cut -d' ' -f1 /proc/uptime 2>/dev/null)
    [ -n "$DIAG_UPTIME" ] || DIAG_UPTIME=unknown

    echo "BUILDSTORM_DIAG_BEGIN phase=$DIAG_PHASE event=$DIAG_EVENT uptime_s=$DIAG_UPTIME"

    if [ -r /proc/meminfo ]; then
        awk '
            /^(MemTotal|MemFree|MemAvailable|Cached|Dirty|Writeback|SwapTotal|SwapFree):/ {
                name=$1
                sub(/:$/, "", name)
                values[name]=$2
                units[name]=$3
            }
            END {
                printf "BUILDSTORM_DIAG_MEM"
                order[1]="MemTotal"; order[2]="MemFree"; order[3]="MemAvailable"
                order[4]="Cached"; order[5]="Dirty"; order[6]="Writeback"
                order[7]="SwapTotal"; order[8]="SwapFree"
                for (i=1; i<=8; i++) {
                    name=order[i]
                    if (name in values) {
                        printf " %s=%s%s", name, values[name], units[name]
                    }
                }
                printf "\n"
            }
        ' /proc/meminfo 2>/dev/null
    else
        echo "BUILDSTORM_DIAG_MEM unavailable"
    fi

    if [ -r /proc/kairix_perf ]; then
        awk '
            /^buildstorm_kernel_diag_version:/ {
                print "BUILDSTORM_DIAG_KERNEL_VERSION " $2
                kernel_version_found=1
                next
            }
            /^(processor_current_tasks|processor_locked|processor_current_samples|processor_current_task_labels|processor_current_syscall_stages|load_balance_ready_tasks|load_balance_online_mask|load_balance_idle_mask|reschedule_ipi_sent|reschedule_ipi_received|task_state_process_table_busy|task_state_process_locks_busy|task_state_first_busy_process_pid|task_state_first_busy_process_owner_cpu|task_state_first_busy_process_owner_line|task_state_task_locks_busy|task_state_total|task_state_ready|task_state_running|task_state_blocked|task_state_zombie|task_state_sleep|task_state_ready_unowned|task_state_running_not_on_cpu|task_state_blocked_queued|task_state_workload_sample_count|task_state_workload_samples|task_state_workload_context_samples|page_cache_pages|page_cache_tmpfs_pages|page_cache_fat32_pages|page_cache_ext4_pages|page_cache_unknown_pages|page_cache_insert_count|page_cache_remove_count|page_cache_lock|lwext4_lock|lwext4_c|ext4_flush|block_io|writeback_pending_files|user_sigill|task_perf):/ {
                print "BUILDSTORM_DIAG_PERF " $0
            }
            END {
                if (!kernel_version_found) {
                    print "BUILDSTORM_DIAG_KERNEL_VERSION unavailable"
                }
            }
        ' /proc/kairix_perf 2>/dev/null
    else
        echo "BUILDSTORM_DIAG_PERF unavailable"
    fi

    if [ -n "$DIAG_TARGET_PID" ] && kill -0 "$DIAG_TARGET_PID" 2>/dev/null; then
        echo "BUILDSTORM_DIAG_TARGET pid=$DIAG_TARGET_PID present=true"
    elif [ -n "$DIAG_TARGET_PID" ]; then
        echo "BUILDSTORM_DIAG_TARGET pid=$DIAG_TARGET_PID present=false"
    else
        echo "BUILDSTORM_DIAG_TARGET pid=none present=false"
    fi

    # Kairix currently reports a placeholder comm and zero CPU accounting in
    # /proc/<pid>/stat. `ps` therefore cannot identify cargo/rustc, and reading
    # every PID can block on a PCB lock. The try-lock workload samples emitted
    # above are the authoritative non-blocking process/task view.
    echo "BUILDSTORM_DIAG_PROCS source=kairix_perf_workload_samples"

    # Do not walk target/debug/deps here. Cargo mutates that ext4 directory
    # concurrently, and a diagnostic `find -printf` can itself block in
    # getdents/stat and prevent all later snapshots. Cargo's crate events plus
    # page-cache, task and process counters provide progress without touching
    # the workload directory.
    echo "BUILDSTORM_DIAG_ARTIFACTS skipped=concurrent_target_walk"

    echo "BUILDSTORM_DIAG_END phase=$DIAG_PHASE event=$DIAG_EVENT uptime_s=$DIAG_UPTIME"
}

buildstorm_diag_failed_exec() {
    DIAG_LOG=$1
    FAILED_EXEC=$(sed -n 's/.*`\([^`]*\/build-script-build\)`.*/\1/p' "$DIAG_LOG" 2>/dev/null | tail -1)
    if [ -z "$FAILED_EXEC" ]; then
        echo "BUILDSTORM_DIAG_FAILED_EXEC path=unavailable"
        return
    fi
    case "$FAILED_EXEC" in
        /work/tgoskits/target/*/build-script-build) ;;
        *)
            echo "BUILDSTORM_DIAG_FAILED_EXEC path=rejected value=$FAILED_EXEC"
            return
            ;;
    esac
    if [ ! -f "$FAILED_EXEC" ]; then
        echo "BUILDSTORM_DIAG_FAILED_EXEC path=$FAILED_EXEC present=false"
        return
    fi

    FAILED_BYTES=$(wc -c < "$FAILED_EXEC" 2>/dev/null)
    FAILED_CKSUM=$(cksum "$FAILED_EXEC" 2>/dev/null | awk '{print $1 ":" $2}')
    FAILED_HEAD=$(od -An -tx1 -N32 "$FAILED_EXEC" 2>/dev/null | tr -d ' \n')
    FAILED_STAT=$(stat -c 'size=%s blocks=%b block_unit=%B inode=%i links=%h' "$FAILED_EXEC" 2>/dev/null)
    echo "BUILDSTORM_DIAG_FAILED_EXEC phase=before_sync path=$FAILED_EXEC present=true bytes=${FAILED_BYTES:-unknown} cksum=${FAILED_CKSUM:-unknown} head=${FAILED_HEAD:-unknown} stat=${FAILED_STAT:-unknown}"
    if command -v readelf >/dev/null 2>&1; then
        readelf -h -A "$FAILED_EXEC" 2>/dev/null \
            | awk '/Class:|Machine:|Type:|Entry point address:|Tag_RISCV_arch:|Tag_LARCH_ARCH:/{print "BUILDSTORM_DIAG_FAILED_ELF " $0}'
    else
        echo "BUILDSTORM_DIAG_FAILED_ELF readelf=unavailable"
    fi

    sed -n 's/.*corrupt metadata encountered in \(\/work\/tgoskits\/target\/[^ ]*\.rmeta\).*/\1/p' "$DIAG_LOG" 2>/dev/null \
        | sort -u \
        | while IFS= read -r FAILED_RMETA; do
            case "$FAILED_RMETA" in
                /work/tgoskits/target/*/deps/*.rmeta) ;;
                *) continue ;;
            esac
            [ -f "$FAILED_RMETA" ] || continue
            RMETA_CKSUM=$(cksum "$FAILED_RMETA" 2>/dev/null | awk '{print $1 ":" $2}')
            RMETA_HEAD=$(od -An -tx1 -N32 "$FAILED_RMETA" 2>/dev/null | tr -d ' \n')
            RMETA_STAT=$(stat -c 'size=%s blocks=%b block_unit=%B inode=%i links=%h' "$FAILED_RMETA" 2>/dev/null)
            echo "BUILDSTORM_DIAG_FAILED_RMETA phase=before_sync path=$FAILED_RMETA cksum=${RMETA_CKSUM:-unknown} head=${RMETA_HEAD:-unknown} stat=${RMETA_STAT:-unknown}"
        done

    sync
    FAILED_CKSUM_AFTER=$(cksum "$FAILED_EXEC" 2>/dev/null | awk '{print $1 ":" $2}')
    FAILED_HEAD_AFTER=$(od -An -tx1 -N32 "$FAILED_EXEC" 2>/dev/null | tr -d ' \n')
    FAILED_STAT_AFTER=$(stat -c 'size=%s blocks=%b block_unit=%B inode=%i links=%h' "$FAILED_EXEC" 2>/dev/null)
    echo "BUILDSTORM_DIAG_FAILED_EXEC phase=after_sync path=$FAILED_EXEC cksum=${FAILED_CKSUM_AFTER:-unknown} head=${FAILED_HEAD_AFTER:-unknown} stat=${FAILED_STAT_AFTER:-unknown}"

    FAILED_REPLAY=/work/buildstorm.failed-exec.replay.out
    timeout 30 sh -c \
        'RUST_BACKTRACE=0 LD_DEBUG=libs,reloc,statistics exec "$1"' \
        buildstorm-replay "$FAILED_EXEC" > "$FAILED_REPLAY" 2>&1
    FAILED_REPLAY_RC=$?
    echo "BUILDSTORM_DIAG_FAILED_EXEC replay_rc=$FAILED_REPLAY_RC"
    tail -20 "$FAILED_REPLAY" 2>/dev/null
}

buildstorm_diag_tmpfs_compile() {
    if [ "$BUILDSTORM_REPLAY_TMPFS_MOUNTED" != true ]; then
        echo "BUILDSTORM_DIAG_TMPFS_COMPILE available=false"
        return
    fi

    TMPFS_BUILD_ROOT=$BUILDSTORM_REPLAY_TMPFS/tg-xtask-build
    TMPFS_TARGET=$TMPFS_BUILD_ROOT/target
    TMPFS_TMPDIR=$TMPFS_BUILD_ROOT/tmp
    TMPFS_LOG=$TMPFS_BUILD_ROOT/cargo.out
    TMPFS_RC_FILE=$TMPFS_BUILD_ROOT/cargo.rc
    rm -rf "$TMPFS_BUILD_ROOT"
    mkdir -p "$TMPFS_TARGET" "$TMPFS_TMPDIR" || {
        echo "BUILDSTORM_DIAG_TMPFS_COMPILE available=false reason=mkdir_failed"
        return
    }

    TMPFS_T0=$(cut -d' ' -f1 /proc/uptime 2>/dev/null)
    [ -n "$TMPFS_T0" ] || TMPFS_T0=0
    echo "BUILDSTORM_DIAG_TMPFS_COMPILE event=start target=$TMPFS_TARGET tmpdir=$TMPFS_TMPDIR"
    BUILDSTORM_DIAG_LOG=$TMPFS_LOG
    {
        buildstorm_run_with_diag tmpfs_tg_xtask timeout 1200 sh -c '
            cd /work/tgoskits || exit 125
            TMPDIR=$1 CARGO_TARGET_DIR=$2 cargo build -p tg-xtask
        ' buildstorm-tmpfs-compile "$TMPFS_TMPDIR" "$TMPFS_TARGET" 2>&1
        echo $? > "$TMPFS_RC_FILE"
    } | tee "$TMPFS_LOG"
    TMPFS_BUILD_RC=$(cat "$TMPFS_RC_FILE" 2>/dev/null || echo 1)
    rm -f "$TMPFS_RC_FILE"
    TMPFS_T1=$(cut -d' ' -f1 /proc/uptime 2>/dev/null)
    [ -n "$TMPFS_T1" ] || TMPFS_T1=$TMPFS_T0
    TMPFS_ELAPSED=$(awk "BEGIN{printf \"%.2f\", (\"$TMPFS_T1\"+0)-(\"$TMPFS_T0\"+0)}" 2>/dev/null)
    [ -n "$TMPFS_ELAPSED" ] || TMPFS_ELAPSED=unknown

    TMPFS_XTASK=$TMPFS_TARGET/debug/tg-xtask
    if [ -f "$TMPFS_XTASK" ]; then
        TMPFS_XTASK_CKSUM=$(cksum "$TMPFS_XTASK" 2>/dev/null | awk '{print $1 ":" $2}')
        TMPFS_XTASK_STAT=$(stat -c 'size=%s blocks=%b block_unit=%B inode=%i links=%h' "$TMPFS_XTASK" 2>/dev/null)
        timeout 30 "$TMPFS_XTASK" --help >/dev/null 2>&1
        TMPFS_EXEC_RC=$?
        if command -v readelf >/dev/null 2>&1; then
            readelf -W -h -l -S "$TMPFS_XTASK" >/dev/null 2>&1
            TMPFS_READELF_RC=$?
        else
            TMPFS_READELF_RC=127
        fi
        echo "BUILDSTORM_DIAG_TMPFS_ARTIFACT present=true cksum=${TMPFS_XTASK_CKSUM:-unknown} stat=${TMPFS_XTASK_STAT:-unknown} readelf_rc=$TMPFS_READELF_RC exec_rc=$TMPFS_EXEC_RC"
    else
        echo "BUILDSTORM_DIAG_TMPFS_ARTIFACT present=false"
    fi
    echo "BUILDSTORM_DIAG_TMPFS_COMPILE event=done rc=$TMPFS_BUILD_RC elapsed_s=$TMPFS_ELAPSED"
    if [ "$TMPFS_BUILD_RC" -ne 0 ]; then
        echo "----- tmpfs tg-xtask build tail -----"
        tail -40 "$TMPFS_LOG" 2>/dev/null
    fi
}

buildstorm_diag_live_failure() {
    DIAG_LOG=$1
    DIAG_KIND=$2
    [ -r "$DIAG_LOG" ] || return

    case "$DIAG_KIND" in
        exec)
            grep -q 'SIGILL: illegal instruction' "$DIAG_LOG" 2>/dev/null || return
            FAILED_EXEC=$(sed -n 's/.*`\([^`]*\/build-script-build\)`.*/\1/p' "$DIAG_LOG" 2>/dev/null | tail -1)
            case "$FAILED_EXEC" in
                /work/tgoskits/target/*/build-script-build) ;;
                *)
                    echo "BUILDSTORM_DIAG_LIVE_EXEC path=unavailable"
                    return
                    ;;
            esac
            if [ ! -f "$FAILED_EXEC" ]; then
                echo "BUILDSTORM_DIAG_LIVE_EXEC path=$FAILED_EXEC present=false"
                return
            fi
            FAILED_CKSUM=$(cksum "$FAILED_EXEC" 2>/dev/null | awk '{print $1 ":" $2}')
            FAILED_HEAD=$(od -An -tx1 -N64 "$FAILED_EXEC" 2>/dev/null | tr -d ' \n')
            FAILED_STAT=$(stat -c 'size=%s blocks=%b block_unit=%B inode=%i links=%h' "$FAILED_EXEC" 2>/dev/null)
            echo "BUILDSTORM_DIAG_LIVE_EXEC path=$FAILED_EXEC present=true cksum=${FAILED_CKSUM:-unknown} head=${FAILED_HEAD:-unknown} stat=${FAILED_STAT:-unknown}"
            if command -v readelf >/dev/null 2>&1; then
                readelf -l "$FAILED_EXEC" 2>/dev/null \
                    | sed -n 's/.*Requesting program interpreter: \(.*\)]/BUILDSTORM_DIAG_LIVE_INTERP path=\1/p'
            fi
            # Replay immediately while the failed inode and its private loader
            # mappings are still the newest relevant state. Do not sync here:
            # the later failure handler deliberately compares before/after
            # sync, while this replay answers whether the crash is already
            # deterministic without waiting for unrelated rustc jobs to exit.
            LIVE_REPLAY=/work/buildstorm.failed-exec.live-replay.out
            timeout 30 sh -c \
                'RUST_BACKTRACE=0 LD_DEBUG=libs,reloc,statistics exec "$1"' \
                buildstorm-live-replay "$FAILED_EXEC" > "$LIVE_REPLAY" 2>&1
            LIVE_REPLAY_RC=$?
            echo "BUILDSTORM_DIAG_LIVE_EXEC phase=before_sync source=ext4 replay_rc=$LIVE_REPLAY_RC"
            tail -40 "$LIVE_REPLAY" 2>/dev/null \
                | sed 's/^/BUILDSTORM_DIAG_LIVE_LD_DEBUG source=ext4 /'

            # A byte-identical tmpfs replay distinguishes corrupt output bytes
            # from incorrect ext4 file-backed exec mappings. tmpfs has its own
            # page-cache namespace and does not reuse the ext4 cache frames.
            LIVE_COPY=$BUILDSTORM_REPLAY_TMPFS/buildstorm.failed-exec.live-copy.$$
            if [ "$BUILDSTORM_REPLAY_TMPFS_MOUNTED" = true ] \
               && cp "$FAILED_EXEC" "$LIVE_COPY" 2>/dev/null \
               && chmod 0700 "$LIVE_COPY" 2>/dev/null; then
                LIVE_COPY_CKSUM=$(cksum "$LIVE_COPY" 2>/dev/null | awk '{print $1 ":" $2}')
                if cmp -s "$FAILED_EXEC" "$LIVE_COPY" 2>/dev/null; then
                    LIVE_COPY_IDENTICAL=true
                else
                    LIVE_COPY_IDENTICAL=false
                fi
                echo "BUILDSTORM_DIAG_LIVE_COPY source=ext4 destination=tmpfs cksum=${LIVE_COPY_CKSUM:-unknown} checksum_matches=$([ "$LIVE_COPY_CKSUM" = "$FAILED_CKSUM" ] && echo true || echo false) byte_identical=$LIVE_COPY_IDENTICAL"
                timeout 30 sh -c \
                    'RUST_BACKTRACE=0 LD_DEBUG=libs,reloc,statistics exec "$1"' \
                    buildstorm-tmpfs-replay "$LIVE_COPY" > "$LIVE_REPLAY.tmpfs" 2>&1
                LIVE_COPY_RC=$?
                echo "BUILDSTORM_DIAG_LIVE_EXEC phase=before_sync source=tmpfs replay_rc=$LIVE_COPY_RC"
                tail -40 "$LIVE_REPLAY.tmpfs" 2>/dev/null \
                    | sed 's/^/BUILDSTORM_DIAG_LIVE_LD_DEBUG source=tmpfs /'
            else
                echo "BUILDSTORM_DIAG_LIVE_COPY source=ext4 destination=tmpfs copy_failed=true"
            fi
            rm -f "$LIVE_COPY"
            ;;
        rmeta)
            grep -q 'corrupt metadata encountered' "$DIAG_LOG" 2>/dev/null || return
            sed -n 's/.*corrupt metadata encountered in \(\/work\/tgoskits\/target\/[^ ]*\.rmeta\).*/\1/p' "$DIAG_LOG" 2>/dev/null \
                | sort -u \
                | while IFS= read -r FAILED_RMETA; do
                    case "$FAILED_RMETA" in
                        /work/tgoskits/target/*/deps/*.rmeta) ;;
                        *) continue ;;
                    esac
                    if [ ! -f "$FAILED_RMETA" ]; then
                        echo "BUILDSTORM_DIAG_LIVE_RMETA path=$FAILED_RMETA present=false"
                        continue
                    fi
                    RMETA_CKSUM=$(cksum "$FAILED_RMETA" 2>/dev/null | awk '{print $1 ":" $2}')
                    RMETA_HEAD=$(od -An -tx1 -N64 "$FAILED_RMETA" 2>/dev/null | tr -d ' \n')
                    RMETA_TAIL=$(tail -c 64 "$FAILED_RMETA" 2>/dev/null | od -An -tx1 | tr -d ' \n')
                    RMETA_STAT=$(stat -c 'size=%s blocks=%b block_unit=%B inode=%i links=%h' "$FAILED_RMETA" 2>/dev/null)
                    echo "BUILDSTORM_DIAG_LIVE_RMETA path=$FAILED_RMETA present=true cksum=${RMETA_CKSUM:-unknown} head=${RMETA_HEAD:-unknown} tail=${RMETA_TAIL:-unknown} stat=${RMETA_STAT:-unknown}"
                done
            ;;
    esac
}

buildstorm_diag_watch() {
    # This watcher is an asynchronous subshell. It must not inherit the parent
    # cleanup trap, otherwise stopping the watcher could also kill the build it
    # is observing.
    trap - EXIT INT TERM
    DIAG_PHASE=$1
    DIAG_WATCH_PID=$2
    DIAG_LOG=$3
    DIAG_SEQ=0
    DIAG_TICKS=0
    DIAG_EXEC_CAPTURED=false
    DIAG_RMETA_CAPTURED=false
    while :; do
        if kill -0 "$DIAG_WATCH_PID" 2>/dev/null; then
            DIAG_TARGET_ALIVE=true
        else
            DIAG_TARGET_ALIVE=false
        fi
        if [ "$DIAG_TICKS" -eq 0 ]; then
            buildstorm_diag_snapshot "$DIAG_PHASE" "sample_$DIAG_SEQ" "$DIAG_WATCH_PID"
            DIAG_SEQ=$((DIAG_SEQ + 1))
        fi
        if [ "$DIAG_EXEC_CAPTURED" = false ] \
           && grep -q 'SIGILL: illegal instruction' "$DIAG_LOG" 2>/dev/null; then
            echo "BUILDSTORM_DIAG_WATCH phase=$DIAG_PHASE event=first_sigill target_pid=$DIAG_WATCH_PID"
            buildstorm_diag_live_failure "$DIAG_LOG" exec
            DIAG_EXEC_CAPTURED=true
        fi
        if [ "$DIAG_RMETA_CAPTURED" = false ] \
           && grep -q 'corrupt metadata encountered' "$DIAG_LOG" 2>/dev/null; then
            echo "BUILDSTORM_DIAG_WATCH phase=$DIAG_PHASE event=first_corrupt_rmeta target_pid=$DIAG_WATCH_PID"
            buildstorm_diag_live_failure "$DIAG_LOG" rmeta
            DIAG_RMETA_CAPTURED=true
        fi
        sleep 1
        DIAG_SLEEP_RC=$?
        if [ "$DIAG_SLEEP_RC" -ne 0 ]; then
            echo "BUILDSTORM_DIAG_WATCH phase=$DIAG_PHASE event=sleep_interrupted target_pid=$DIAG_WATCH_PID rc=$DIAG_SLEEP_RC"
        fi
        DIAG_TICKS=$((DIAG_TICKS + 1))
        if [ "$DIAG_TICKS" -ge "$BUILDSTORM_DIAG_INTERVAL" ]; then
            DIAG_TICKS=0
        fi
    done
}

buildstorm_stop_running() {
    if [ -n "$BUILDSTORM_MONITOR_PID" ]; then
        kill "$BUILDSTORM_MONITOR_PID" 2>/dev/null || true
        wait "$BUILDSTORM_MONITOR_PID" 2>/dev/null || true
        BUILDSTORM_MONITOR_PID=
    fi
    if [ -n "$BUILDSTORM_ACTIVE_PID" ]; then
        kill "$BUILDSTORM_ACTIVE_PID" 2>/dev/null || true
        wait "$BUILDSTORM_ACTIVE_PID" 2>/dev/null || true
        BUILDSTORM_ACTIVE_PID=
    fi
    if [ "$BUILDSTORM_REPLAY_TMPFS_MOUNTED" = true ]; then
        umount "$BUILDSTORM_REPLAY_TMPFS" 2>/dev/null || true
        BUILDSTORM_REPLAY_TMPFS_MOUNTED=false
    fi
}

buildstorm_run_with_diag() {
    DIAG_PHASE=$1
    shift

    DIAG_T0=$(cut -d' ' -f1 /proc/uptime 2>/dev/null)
    "$@" &
    BUILDSTORM_ACTIVE_PID=$!
    DIAG_COMMAND_PID=$BUILDSTORM_ACTIVE_PID
    echo "BUILDSTORM_DIAG_COMMAND phase=$DIAG_PHASE event=start pid=$BUILDSTORM_ACTIVE_PID command=$*"

    buildstorm_diag_watch "$DIAG_PHASE" "$BUILDSTORM_ACTIVE_PID" "$BUILDSTORM_DIAG_LOG" &
    BUILDSTORM_MONITOR_PID=$!

    wait "$BUILDSTORM_ACTIVE_PID"
    DIAG_RC=$?
    BUILDSTORM_ACTIVE_PID=

    kill "$BUILDSTORM_MONITOR_PID" 2>/dev/null || true
    wait "$BUILDSTORM_MONITOR_PID" 2>/dev/null || true
    BUILDSTORM_MONITOR_PID=

    DIAG_T1=$(cut -d' ' -f1 /proc/uptime 2>/dev/null)
    DIAG_ELAPSED=$(awk "BEGIN{printf \"%.2f\", (\"$DIAG_T1\"+0)-(\"$DIAG_T0\"+0)}" 2>/dev/null)
    [ -n "$DIAG_ELAPSED" ] || DIAG_ELAPSED=unknown
    buildstorm_diag_snapshot "$DIAG_PHASE" final "$DIAG_COMMAND_PID"
    echo "BUILDSTORM_DIAG_COMMAND phase=$DIAG_PHASE event=done pid=none rc=$DIAG_RC elapsed_s=$DIAG_ELAPSED"
    return "$DIAG_RC"
}

buildstorm_minibuild_at() {
    MINIBUILD_SOURCE=$1
    MINIBUILD_PATH=$2

    echo "BUILDSTORM_DIAG_MINIBUILD source=$MINIBUILD_SOURCE stage=cleanup event=start path=$MINIBUILD_PATH"
    timeout 120 rm -rf "$MINIBUILD_PATH"
    MINIBUILD_STAGE_RC=$?
    echo "BUILDSTORM_DIAG_MINIBUILD source=$MINIBUILD_SOURCE stage=cleanup event=done rc=$MINIBUILD_STAGE_RC"
    [ "$MINIBUILD_STAGE_RC" -eq 0 ] || return "$MINIBUILD_STAGE_RC"

    echo "BUILDSTORM_DIAG_MINIBUILD source=$MINIBUILD_SOURCE stage=cargo_new event=start"
    timeout 120 cargo new --vcs none "$MINIBUILD_PATH"
    MINIBUILD_STAGE_RC=$?
    echo "BUILDSTORM_DIAG_MINIBUILD source=$MINIBUILD_SOURCE stage=cargo_new event=done rc=$MINIBUILD_STAGE_RC"
    [ "$MINIBUILD_STAGE_RC" -eq 0 ] || return "$MINIBUILD_STAGE_RC"

    echo "BUILDSTORM_DIAG_MINIBUILD source=$MINIBUILD_SOURCE stage=cargo_build event=start"
    (
        cd "$MINIBUILD_PATH" || exit 125
        timeout 600 cargo build
    )
    MINIBUILD_STAGE_RC=$?
    echo "BUILDSTORM_DIAG_MINIBUILD source=$MINIBUILD_SOURCE stage=cargo_build event=done rc=$MINIBUILD_STAGE_RC"
    [ "$MINIBUILD_STAGE_RC" -eq 0 ] || return "$MINIBUILD_STAGE_RC"

    echo "BUILDSTORM_DIAG_MINIBUILD source=$MINIBUILD_SOURCE stage=execute event=start"
    MINIBUILD_OUTPUT=$(timeout 30 "$MINIBUILD_PATH/target/debug/minibuild" 2>&1)
    MINIBUILD_STAGE_RC=$?
    if [ "$MINIBUILD_OUTPUT" = "Hello, world!" ]; then
        MINIBUILD_OUTPUT_OK=true
    else
        MINIBUILD_OUTPUT_OK=false
    fi
    echo "BUILDSTORM_DIAG_MINIBUILD source=$MINIBUILD_SOURCE stage=execute event=done rc=$MINIBUILD_STAGE_RC output_ok=$MINIBUILD_OUTPUT_OK"
    [ "$MINIBUILD_STAGE_RC" -eq 0 ] && [ "$MINIBUILD_OUTPUT_OK" = true ]
}

buildstorm_run_minibuild() {
    MINIBUILD_SOURCE=$1
    MINIBUILD_PATH=$2
    MINIBUILD_LOG=$3
    MINIBUILD_RC_FILE=$4

    BUILDSTORM_DIAG_LOG=$MINIBUILD_LOG
    {
        buildstorm_run_with_diag "minibuild_$MINIBUILD_SOURCE" \
            buildstorm_minibuild_at "$MINIBUILD_SOURCE" "$MINIBUILD_PATH" 2>&1
        echo $? > "$MINIBUILD_RC_FILE"
    } | tee "$MINIBUILD_LOG"
    MINIBUILD_RC=$(cat "$MINIBUILD_RC_FILE" 2>/dev/null || echo 1)
    rm -f "$MINIBUILD_RC_FILE"
    return "$MINIBUILD_RC"
}

trap 'buildstorm_stop_running; exit 130' INT
trap 'buildstorm_stop_running; exit 143' TERM
trap 'buildstorm_stop_running' EXIT

case "$(uname -m 2>/dev/null)" in
  loongarch64) AXARCH=loongarch64; AXTGT=loongarch64-unknown-linux-musl ;;
  riscv64)     AXARCH=riscv64;     AXTGT=riscv64gc-unknown-linux-musl ;;
  *)           AXARCH=riscv64;     AXTGT=riscv64gc-unknown-linux-musl ;;
esac

if rustc --version && cargo --version; then
    echo "BUILDSTORM_TOOLCHAIN ok"
else
    echo "BUILDSTORM_TOOLCHAIN fail"
fi

MINIBUILD_LOG=$BUILDSTORM_REPLAY_TMPFS/buildstorm.minibuild.ext4.out
MINIBUILD_RC_FILE=$BUILDSTORM_REPLAY_TMPFS/buildstorm.minibuild.ext4.rc
if buildstorm_run_minibuild ext4 /tmp/minibuild "$MINIBUILD_LOG" "$MINIBUILD_RC_FILE"; then
    echo "BUILDSTORM_MINIBUILD ok"
else
    MINIBUILD_EXT4_RC=$?
    echo "BUILDSTORM_DIAG_MINIBUILD source=ext4 event=failed rc=$MINIBUILD_EXT4_RC"
    if [ "$BUILDSTORM_REPLAY_TMPFS_MOUNTED" = true ]; then
        MINIBUILD_TMPFS_PATH=$BUILDSTORM_REPLAY_TMPFS/minibuild
        MINIBUILD_TMPFS_LOG=$BUILDSTORM_REPLAY_TMPFS/buildstorm.minibuild.tmpfs.out
        MINIBUILD_TMPFS_RC_FILE=$BUILDSTORM_REPLAY_TMPFS/buildstorm.minibuild.tmpfs.rc
        if buildstorm_run_minibuild tmpfs "$MINIBUILD_TMPFS_PATH" \
            "$MINIBUILD_TMPFS_LOG" "$MINIBUILD_TMPFS_RC_FILE"; then
            echo "BUILDSTORM_DIAG_MINIBUILD source=tmpfs event=differential_pass"
        else
            MINIBUILD_TMPFS_RC=$?
            echo "BUILDSTORM_DIAG_MINIBUILD source=tmpfs event=differential_fail rc=$MINIBUILD_TMPFS_RC"
        fi
    else
        echo "BUILDSTORM_DIAG_MINIBUILD source=tmpfs event=unavailable"
    fi
    echo "BUILDSTORM_MINIBUILD fail"
fi

if [ -x /vfork_exec_failure_test ]; then
    echo "----- vfork exec-failure regression (untimed) -----"
    timeout 120 /vfork_exec_failure_test
    VFORK_TEST_RC=$?
    if [ "$VFORK_TEST_RC" -eq 0 ]; then
        echo "BUILDSTORM_VFORK_EXEC_FAILURE ok"
    else
        echo "BUILDSTORM_VFORK_EXEC_FAILURE fail rc=$VFORK_TEST_RC"
    fi
else
    echo "BUILDSTORM_VFORK_EXEC_FAILURE unavailable"
fi

if [ -x /ext4_exec_coherence_test ]; then
    echo "----- ext4 mmap/writeback coherence regression (untimed) -----"
    timeout 180 /ext4_exec_coherence_test
    EXT4_COHERENCE_RC=$?
    if [ "$EXT4_COHERENCE_RC" -eq 0 ]; then
        echo "BUILDSTORM_EXT4_COHERENCE ok"
    else
        echo "BUILDSTORM_EXT4_COHERENCE fail rc=$EXT4_COHERENCE_RC"
    fi
else
    echo "BUILDSTORM_EXT4_COHERENCE unavailable"
fi

if [ -x /ext4_linker_sparse_write_test ]; then
    echo "----- ext4 linker/tmpfile regression (untimed) -----"
    timeout 240 /ext4_linker_sparse_write_test
    EXT4_LINKER_RC=$?
    if [ "$EXT4_LINKER_RC" -eq 0 ]; then
        echo "BUILDSTORM_EXT4_LINKER ok"
    else
        echo "BUILDSTORM_EXT4_LINKER fail rc=$EXT4_LINKER_RC"
    fi
else
    echo "BUILDSTORM_EXT4_LINKER unavailable"
fi

if [ -x /ext4_exit_publish_test ]; then
    echo "----- ext4 exit/publication regression (untimed) -----"
    timeout 240 /ext4_exit_publish_test
    EXT4_EXIT_PUBLISH_RC=$?
    if [ "$EXT4_EXIT_PUBLISH_RC" -eq 0 ]; then
        echo "BUILDSTORM_EXT4_EXIT_PUBLISH ok"
    else
        echo "BUILDSTORM_EXT4_EXIT_PUBLISH fail rc=$EXT4_EXIT_PUBLISH_RC"
    fi
else
    echo "BUILDSTORM_EXT4_EXIT_PUBLISH unavailable"
fi

cd /work/tgoskits 2>/dev/null || {
    echo "BUILDSTORM_COMPILE mode=multi ok=false elapsed_s=0 cores=$(nproc) bytes=0 arch=$AXARCH"
    echo "#### OS COMP TEST GROUP END buildstorm ####"
    exit 1
}

# `cargo build -p tg-xtask` is a host build and lives under target/debug, not
# target/$AXTGT.  Cargo freshness checks do not validate the contents of an
# existing ELF or rmeta.  Consequently a corrupt artifact from an earlier run
# is otherwise accepted forever, which is exactly what the repeated inode and
# checksum in the failure diagnostics showed.  Make this phase genuinely fresh
# and monitor the potentially long ext4 directory deletion as well.
echo "----- clean host and target artifacts (untimed) -----"
BUILDSTORM_DIAG_LOG=/work/buildstorm.target-clean.out
{
    buildstorm_run_with_diag target_cleanup timeout 1200 rm -rf target/debug "target/$AXTGT" 2>&1
    echo $? > /work/.buildstorm.target-clean.rc
} | tee /work/buildstorm.target-clean.out
TARGET_CLEAN_RC=$(cat /work/.buildstorm.target-clean.rc 2>/dev/null || echo 1)
rm -f /work/.buildstorm.target-clean.rc
if [ "$TARGET_CLEAN_RC" -ne 0 ]; then
    echo "BUILDSTORM_DIAG_TARGET_CLEAN ok=false rc=$TARGET_CLEAN_RC"
    echo "BUILDSTORM_COMPILE mode=multi ok=false rc=$TARGET_CLEAN_RC elapsed_s=0 cores=$(nproc) bytes=0 arch=$AXARCH"
    echo "#### OS COMP TEST GROUP END buildstorm ####"
    sync
    exit 1
fi
if [ -e target/debug ] || [ -e "target/$AXTGT" ]; then
    echo "BUILDSTORM_DIAG_TARGET_CLEAN ok=false rc=1 reason=path_still_present"
    echo "BUILDSTORM_COMPILE mode=multi ok=false rc=1 elapsed_s=0 cores=$(nproc) bytes=0 arch=$AXARCH"
    echo "#### OS COMP TEST GROUP END buildstorm ####"
    sync
    exit 1
fi
echo "BUILDSTORM_DIAG_TARGET_CLEAN ok=true host=target/debug target=target/$AXTGT"

echo "----- pre-build tg-xtask (untimed) -----"
BUILDSTORM_DIAG_LOG=/work/buildstorm.xtask.out
{
    buildstorm_run_with_diag tg_xtask cargo build -p tg-xtask 2>&1
    echo $? > /work/.buildstorm.xtask.rc
} | tee /work/buildstorm.xtask.out
XTASK_RC=$(cat /work/.buildstorm.xtask.rc 2>/dev/null || echo 1)
rm -f /work/.buildstorm.xtask.rc
if [ "$XTASK_RC" -ne 0 ]; then
    echo "BUILDSTORM_DIAG_COMMAND phase=tg_xtask event=failed rc=$XTASK_RC"
    buildstorm_diag_failed_exec /work/buildstorm.xtask.out
    buildstorm_diag_snapshot tg_xtask failed_exec_replay none
    echo "----- tmpfs tg-xtask differential (diagnostic, untimed) -----"
    buildstorm_diag_tmpfs_compile
    # Do not invoke a missing or corrupt xtask executable.  Preserve the
    # contestant-facing result line while keeping this failure attributed to
    # the host-tool build that actually failed.
    echo "BUILDSTORM_COMPILE mode=multi ok=false rc=$XTASK_RC elapsed_s=0 cores=$(nproc) bytes=0 arch=$AXARCH"
    echo "#### OS COMP TEST GROUP END buildstorm ####"
    sync
    exit 1
fi

echo "----- build arceos-helloworld (timed, arch=$AXARCH) -----"
BUILDSTORM_DIAG_LOG=/work/buildstorm.build.out
echo "BUILDSTORM_BEGIN mode=multi"
T0=$(cut -d' ' -f1 /proc/uptime 2>/dev/null)
{
    buildstorm_run_with_diag arceos_build timeout 14400 cargo xtask arceos build -p arceos-helloworld --arch "$AXARCH" 2>&1
    echo $? > /work/.build.rc
} | tee /work/buildstorm.build.out
RC=$(cat /work/.build.rc 2>/dev/null || echo 1)
rm -f /work/.build.rc
T1=$(cut -d' ' -f1 /proc/uptime 2>/dev/null)
ELAPSED=$(awk "BEGIN{printf \"%.2f\", (\"$T1\"+0)-(\"$T0\"+0)}" 2>/dev/null)
[ -z "$ELAPSED" ] && ELAPSED=0

ART=$(find target -type f \( -name 'arceos-helloworld' -o -name 'helloworld' \) 2>/dev/null | head -1)
BYTES=0
[ -n "$ART" ] && BYTES=$(wc -c < "$ART")

if [ "$RC" -eq 0 ] && [ -n "$ART" ] && [ "$BYTES" -ge 500000 ]; then
    echo "BUILDSTORM_COMPILE mode=multi ok=true elapsed_s=$ELAPSED cores=$(nproc) bytes=$BYTES arch=$AXARCH"
else
    echo "BUILDSTORM_COMPILE mode=multi ok=false rc=$RC elapsed_s=$ELAPSED cores=$(nproc) bytes=$BYTES arch=$AXARCH"
    echo "----- buildstorm.build.out tail -----"
    tail -25 /work/buildstorm.build.out 2>/dev/null
fi

echo "#### OS COMP TEST GROUP END buildstorm ####"
sync
