#!/bin/sh
# BuildStorm testcode. The score-facing result lines remain unchanged; the
# additional BUILDSTORM_GLOBAL lines attribute elapsed CPU capacity globally
# to user mode, kernel mode and scheduler idle time.

echo "#### OS COMP TEST GROUP START buildstorm ####"
echo "BUILDSTORM_DIAG_VERSION 2026-08-02.13"
echo "BUILDSTORM_BUILD_SEMANTICS original-image-cache-policy"

mount -t proc proc /proc 2>/dev/null
mount -t sysfs sysfs /sys 2>/dev/null
mount -t devtmpfs devtmpfs /dev 2>/dev/null

export PATH=/root/.cargo/bin:/usr/local/bin:/usr/bin:/bin:/sbin:/usr/sbin
export HOME=/root RUSTUP_HOME=/root/.rustup CARGO_HOME=/root/.cargo
export RUSTUP_TOOLCHAIN=nightly-2026-05-28
export CARGO_NET_OFFLINE=true

BUILDSTORM_GLOBAL_INTERVAL=30
BUILDSTORM_ACTIVE_PID=
BUILDSTORM_MONITOR_PID=

buildstorm_read_global() {
    [ -r /proc/kairix_perf ] || return 1
    GLOBAL_VALUES=$(awk '
        /^global_cpu_time:/ {
            for (i = 2; i <= NF; i++) {
                split($i, pair, "=")
                value[pair[1]] = pair[2]
            }
            global_found = 1
        }
        /^heap_perf:/ {
            for (i = 2; i <= NF; i++) {
                split($i, pair, "=")
                heap[pair[1]] = pair[2]
            }
            heap_found = 1
        }
        END {
            if (!global_found || !heap_found) {
                exit 1
            }
            print value["now_ns"], value["online_cpus"], value["capacity_ns"], \
                  value["user_ns"], value["kernel_ns"], value["idle_ns"], \
                  heap["cache_hits"], heap["cache_misses"], \
                  heap["global_alloc_blocks"], heap["global_dealloc_blocks"], \
                  heap["lock_acquisitions"], heap["lock_contended"], \
                  heap["lock_wait_ns"], heap["lock_max_wait_ns"], \
                  heap["contended_alloc"], heap["contended_dealloc"], \
                  heap["contended_refill"], heap["contended_drain"]
        }
    ' /proc/kairix_perf 2>/dev/null) || return 1
    set -- $GLOBAL_VALUES
    [ "$#" -eq 18 ] || return 1
    GLOBAL_NOW_NS=$1
    GLOBAL_ONLINE_CPUS=$2
    GLOBAL_CAPACITY_NS=$3
    GLOBAL_USER_NS=$4
    GLOBAL_KERNEL_NS=$5
    GLOBAL_IDLE_NS=$6
    GLOBAL_HEAP_CACHE_HITS=$7
    GLOBAL_HEAP_CACHE_MISSES=$8
    GLOBAL_HEAP_GLOBAL_ALLOCS=$9
    shift 9
    GLOBAL_HEAP_GLOBAL_DEALLOCS=$1
    GLOBAL_HEAP_LOCK_ACQUISITIONS=$2
    GLOBAL_HEAP_LOCK_CONTENDED=$3
    GLOBAL_HEAP_LOCK_WAIT_NS=$4
    GLOBAL_HEAP_LOCK_MAX_WAIT_NS=$5
    GLOBAL_HEAP_CONTENDED_ALLOC=$6
    GLOBAL_HEAP_CONTENDED_DEALLOC=$7
    GLOBAL_HEAP_CONTENDED_REFILL=$8
    GLOBAL_HEAP_CONTENDED_DRAIN=$9
}

buildstorm_global_snapshot() {
    SNAP_PHASE=$1
    SNAP_EVENT=$2
    SNAP_PHASE_NOW=$3
    SNAP_PHASE_CAPACITY=$4
    SNAP_PHASE_USER=$5
    SNAP_PHASE_KERNEL=$6
    SNAP_PHASE_IDLE=$7
    SNAP_OVERALL_NOW=$8
    SNAP_OVERALL_CAPACITY=$9
    shift 9
    SNAP_OVERALL_USER=$1
    SNAP_OVERALL_KERNEL=$2
    SNAP_OVERALL_IDLE=$3

    if ! buildstorm_read_global; then
        echo "BUILDSTORM_GLOBAL phase=$SNAP_PHASE event=$SNAP_EVENT unavailable=true"
        return
    fi

    awk \
        -v phase="$SNAP_PHASE" \
        -v event="$SNAP_EVENT" \
        -v cpus="$GLOBAL_ONLINE_CPUS" \
        -v now="$GLOBAL_NOW_NS" \
        -v cap="$GLOBAL_CAPACITY_NS" \
        -v user="$GLOBAL_USER_NS" \
        -v kernel="$GLOBAL_KERNEL_NS" \
        -v idle="$GLOBAL_IDLE_NS" \
        -v heap_cache_hits="$GLOBAL_HEAP_CACHE_HITS" \
        -v heap_cache_misses="$GLOBAL_HEAP_CACHE_MISSES" \
        -v heap_global_allocs="$GLOBAL_HEAP_GLOBAL_ALLOCS" \
        -v heap_global_deallocs="$GLOBAL_HEAP_GLOBAL_DEALLOCS" \
        -v heap_lock_acquisitions="$GLOBAL_HEAP_LOCK_ACQUISITIONS" \
        -v heap_lock_contended="$GLOBAL_HEAP_LOCK_CONTENDED" \
        -v heap_lock_wait_ns="$GLOBAL_HEAP_LOCK_WAIT_NS" \
        -v heap_lock_max_wait_ns="$GLOBAL_HEAP_LOCK_MAX_WAIT_NS" \
        -v heap_contended_alloc="$GLOBAL_HEAP_CONTENDED_ALLOC" \
        -v heap_contended_dealloc="$GLOBAL_HEAP_CONTENDED_DEALLOC" \
        -v heap_contended_refill="$GLOBAL_HEAP_CONTENDED_REFILL" \
        -v heap_contended_drain="$GLOBAL_HEAP_CONTENDED_DRAIN" \
        -v pnow="$SNAP_PHASE_NOW" \
        -v pcap="$SNAP_PHASE_CAPACITY" \
        -v puser="$SNAP_PHASE_USER" \
        -v pkernel="$SNAP_PHASE_KERNEL" \
        -v pidle="$SNAP_PHASE_IDLE" \
        -v onow="$SNAP_OVERALL_NOW" \
        -v ocap="$SNAP_OVERALL_CAPACITY" \
        -v ouser="$SNAP_OVERALL_USER" \
        -v okernel="$SNAP_OVERALL_KERNEL" \
        -v oidle="$SNAP_OVERALL_IDLE" '
        function nonnegative(value) {
            return value < 0 ? 0 : value
        }
        BEGIN {
            wall = nonnegative(now - pnow)
            capacity = nonnegative(cap - pcap)
            user_delta = nonnegative(user - puser)
            kernel_delta = nonnegative(kernel - pkernel)
            idle_delta = nonnegative(idle - pidle)

            overall_wall = nonnegative(now - onow)
            overall_capacity = nonnegative(cap - ocap)
            overall_user = nonnegative(user - ouser)
            overall_kernel = nonnegative(kernel - okernel)
            overall_idle = nonnegative(idle - oidle)

            user_pct = capacity ? user_delta * 100 / capacity : 0
            kernel_pct = capacity ? kernel_delta * 100 / capacity : 0
            idle_pct = capacity ? idle_delta * 100 / capacity : 0
            busy_cpus = wall ? (user_delta + kernel_delta) / wall : 0
            user_cpus = wall ? user_delta / wall : 0
            kernel_cpus = wall ? kernel_delta / wall : 0

            overall_user_pct = overall_capacity ? overall_user * 100 / overall_capacity : 0
            overall_kernel_pct = overall_capacity ? overall_kernel * 100 / overall_capacity : 0
            overall_idle_pct = overall_capacity ? overall_idle * 100 / overall_capacity : 0
            overall_busy_cpus = overall_wall ? (overall_user + overall_kernel) / overall_wall : 0

            printf "BUILDSTORM_GLOBAL phase=%s event=%s online_cpus=%d", phase, event, cpus
            printf " wall_s=%.2f capacity_s=%.2f user_s=%.2f kernel_s=%.2f idle_s=%.2f", \
                wall / 1000000000, capacity / 1000000000, user_delta / 1000000000, \
                kernel_delta / 1000000000, idle_delta / 1000000000
            printf " user_pct=%.2f kernel_pct=%.2f idle_pct=%.2f", \
                user_pct, kernel_pct, idle_pct
            printf " user_cpus=%.2f kernel_cpus=%.2f busy_cpus=%.2f", \
                user_cpus, kernel_cpus, busy_cpus
            printf " overall_wall_s=%.2f overall_user_pct=%.2f overall_kernel_pct=%.2f overall_idle_pct=%.2f overall_busy_cpus=%.2f", \
                overall_wall / 1000000000, overall_user_pct, overall_kernel_pct, \
                overall_idle_pct, overall_busy_cpus
            printf " heap_cache_hits=%s heap_cache_misses=%s heap_global_allocs=%s heap_global_deallocs=%s", \
                heap_cache_hits, heap_cache_misses, heap_global_allocs, heap_global_deallocs
            printf " heap_lock_acquisitions=%s heap_lock_contended=%s heap_lock_wait_ns=%s heap_lock_max_wait_ns=%s", \
                heap_lock_acquisitions, heap_lock_contended, heap_lock_wait_ns, heap_lock_max_wait_ns
            printf " heap_contended_alloc=%s heap_contended_dealloc=%s heap_contended_refill=%s heap_contended_drain=%s\n", \
                heap_contended_alloc, heap_contended_dealloc, heap_contended_refill, heap_contended_drain
        }
    '
}

buildstorm_global_watch() {
    WATCH_PHASE=$1
    WATCH_PID=$2
    shift 2
    WATCH_SEQUENCE=0
    while kill -0 "$WATCH_PID" 2>/dev/null; do
        buildstorm_global_snapshot "$WATCH_PHASE" "sample_$WATCH_SEQUENCE" "$@"
        WATCH_SEQUENCE=$((WATCH_SEQUENCE + 1))
        sleep "$BUILDSTORM_GLOBAL_INTERVAL" || break
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
}

buildstorm_run_global() {
    RUN_PHASE=$1
    shift

    if ! buildstorm_read_global; then
        echo "BUILDSTORM_GLOBAL phase=$RUN_PHASE event=start unavailable=true"
        "$@"
        return $?
    fi

    RUN_NOW=$GLOBAL_NOW_NS
    RUN_CAPACITY=$GLOBAL_CAPACITY_NS
    RUN_USER=$GLOBAL_USER_NS
    RUN_KERNEL=$GLOBAL_KERNEL_NS
    RUN_IDLE=$GLOBAL_IDLE_NS

    "$@" &
    BUILDSTORM_ACTIVE_PID=$!
    buildstorm_global_watch "$RUN_PHASE" "$BUILDSTORM_ACTIVE_PID" \
        "$RUN_NOW" "$RUN_CAPACITY" "$RUN_USER" "$RUN_KERNEL" "$RUN_IDLE" \
        "$OVERALL_NOW" "$OVERALL_CAPACITY" "$OVERALL_USER" \
        "$OVERALL_KERNEL" "$OVERALL_IDLE" &
    BUILDSTORM_MONITOR_PID=$!

    wait "$BUILDSTORM_ACTIVE_PID"
    RUN_RC=$?
    BUILDSTORM_ACTIVE_PID=

    kill "$BUILDSTORM_MONITOR_PID" 2>/dev/null || true
    wait "$BUILDSTORM_MONITOR_PID" 2>/dev/null || true
    BUILDSTORM_MONITOR_PID=

    buildstorm_global_snapshot "$RUN_PHASE" final \
        "$RUN_NOW" "$RUN_CAPACITY" "$RUN_USER" "$RUN_KERNEL" "$RUN_IDLE" \
        "$OVERALL_NOW" "$OVERALL_CAPACITY" "$OVERALL_USER" \
        "$OVERALL_KERNEL" "$OVERALL_IDLE"
    return "$RUN_RC"
}

buildstorm_minibuild() {
    timeout 120 rm -rf /tmp/minibuild || return $?
    timeout 120 cargo new --vcs none /tmp/minibuild || return $?
    (
        cd /tmp/minibuild || exit 125
        timeout 600 cargo build
    ) || return $?
    MINIBUILD_OUTPUT=$(timeout 30 /tmp/minibuild/target/debug/minibuild 2>&1)
    [ "$?" -eq 0 ] && [ "$MINIBUILD_OUTPUT" = "Hello, world!" ]
}

trap 'buildstorm_stop_running; exit 130' INT
trap 'buildstorm_stop_running; exit 143' TERM
trap 'buildstorm_stop_running' EXIT

case "$(uname -m 2>/dev/null)" in
    loongarch64) AXARCH=loongarch64; AXTGT=loongarch64-unknown-linux-musl ;;
    riscv64) AXARCH=riscv64; AXTGT=riscv64gc-unknown-linux-musl ;;
    *) AXARCH=riscv64; AXTGT=riscv64gc-unknown-linux-musl ;;
esac

if buildstorm_read_global; then
    OVERALL_NOW=$GLOBAL_NOW_NS
    OVERALL_CAPACITY=$GLOBAL_CAPACITY_NS
    OVERALL_USER=$GLOBAL_USER_NS
    OVERALL_KERNEL=$GLOBAL_KERNEL_NS
    OVERALL_IDLE=$GLOBAL_IDLE_NS
    KERNEL_DIAG_VERSION=$(awk '/^buildstorm_kernel_diag_version:/ { print $2; exit }' /proc/kairix_perf)
    echo "BUILDSTORM_DIAG_KERNEL_VERSION ${KERNEL_DIAG_VERSION:-unavailable}"
else
    OVERALL_NOW=0
    OVERALL_CAPACITY=0
    OVERALL_USER=0
    OVERALL_KERNEL=0
    OVERALL_IDLE=0
    echo "BUILDSTORM_DIAG_KERNEL_VERSION unavailable"
fi

if rustc --version && cargo --version; then
    echo "BUILDSTORM_TOOLCHAIN ok"
else
    echo "BUILDSTORM_TOOLCHAIN fail"
fi

if buildstorm_run_global minibuild buildstorm_minibuild; then
    echo "BUILDSTORM_MINIBUILD ok"
else
    echo "BUILDSTORM_MINIBUILD fail"
fi

cd /work/tgoskits 2>/dev/null || {
    echo "BUILDSTORM_COMPILE mode=multi ok=false elapsed_s=0 cores=$(nproc) bytes=0 arch=$AXARCH"
    echo "#### OS COMP TEST GROUP END buildstorm ####"
    exit 1
}

# Match the published image's build policy: rebuild the architecture target,
# but preserve target/debug so the prebuilt host tg-xtask and its dependencies
# remain reusable by Cargo's freshness check.
rm -rf "target/$AXTGT"

echo "----- pre-build tg-xtask -----"
{
    buildstorm_run_global tg_xtask cargo build -p tg-xtask 2>&1
    echo $? > /work/.buildstorm.xtask.rc
} | tee /work/buildstorm.xtask.out
XTASK_RC=$(cat /work/.buildstorm.xtask.rc 2>/dev/null || echo 1)
rm -f /work/.buildstorm.xtask.rc
if [ "$XTASK_RC" -ne 0 ]; then
    echo "BUILDSTORM_COMPILE mode=multi ok=false rc=$XTASK_RC elapsed_s=0 cores=$(nproc) bytes=0 arch=$AXARCH"
    echo "#### OS COMP TEST GROUP END buildstorm ####"
    exit 1
fi

echo "----- build arceos-helloworld (timed, arch=$AXARCH) -----"
echo "BUILDSTORM_BEGIN mode=multi"
T0=$(cut -d' ' -f1 /proc/uptime 2>/dev/null)
{
    buildstorm_run_global arceos_build timeout 14400 \
        cargo xtask arceos build -p arceos-helloworld --arch "$AXARCH" 2>&1
    echo $? > /work/.build.rc
} | tee /work/buildstorm.build.out
RC=$(cat /work/.build.rc 2>/dev/null || echo 1)
rm -f /work/.build.rc
T1=$(cut -d' ' -f1 /proc/uptime 2>/dev/null)
ELAPSED=$(awk "BEGIN{printf \"%.2f\", (\"$T1\"+0)-(\"$T0\"+0)}" 2>/dev/null)
[ -n "$ELAPSED" ] || ELAPSED=0

ART=$(find target -type f \( -name arceos-helloworld -o -name helloworld \) 2>/dev/null | head -1)
BYTES=0
[ -n "$ART" ] && BYTES=$(wc -c < "$ART")

if [ "$RC" -eq 0 ] && [ -n "$ART" ] && [ "$BYTES" -ge 500000 ]; then
    echo "BUILDSTORM_COMPILE mode=multi ok=true elapsed_s=$ELAPSED cores=$(nproc) bytes=$BYTES arch=$AXARCH"
else
    echo "BUILDSTORM_COMPILE mode=multi ok=false rc=$RC elapsed_s=$ELAPSED cores=$(nproc) bytes=$BYTES arch=$AXARCH"
    tail -25 /work/buildstorm.build.out 2>/dev/null
fi

buildstorm_global_snapshot overall final \
    "$OVERALL_NOW" "$OVERALL_CAPACITY" "$OVERALL_USER" \
    "$OVERALL_KERNEL" "$OVERALL_IDLE" \
    "$OVERALL_NOW" "$OVERALL_CAPACITY" "$OVERALL_USER" \
    "$OVERALL_KERNEL" "$OVERALL_IDLE"
echo "#### OS COMP TEST GROUP END buildstorm ####"
