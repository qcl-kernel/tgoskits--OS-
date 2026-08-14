#include <finsh.h>
#include <stdint.h>
#include <stdlib.h>

#include <drivers/ofw.h>
#include <rthw.h>
#include <rtthread.h>

#define RTPROBE_MAX_SAMPLES 120000
#define RTPROBE_RECORDED_MISSES 16
#define RTPROBE_RECORDED_INTERVALS 16
#define RTPROBE_STACK_SIZE 4096
#define RTPROBE_PRIORITY 5
#define RTPROBE_AUTORUN_PRIORITY 10
#define RTPROBE_STRESS_PRIORITY 20
#define RTPROBE_STRESS_WORKERS 2
#define RTPROBE_STRESS_WORDS 16384
#define RTPROBE_IDLE_SAMPLES 6000
#define RTPROBE_LONG_STRESS_SAMPLES 60000

static uint64_t jitter_ns[RTPROBE_MAX_SAMPLES];
static volatile uint32_t stress_words[RTPROBE_STRESS_WORKERS][RTPROBE_STRESS_WORDS];

struct rtprobe_result
{
    int samples;
    uint64_t missed;
    uint64_t sum;
    uint64_t max;
    uint64_t p99;
    uint64_t p999;
    uint64_t actual_min;
    uint64_t actual_max;
    uint64_t below_half_period;
    uint64_t above_three_halves;
    int recorded_intervals;
    uint64_t actual_interval_ns[RTPROBE_RECORDED_INTERVALS];
    int tracks_tick_advance;
    uint64_t tick_advance_zero;
    uint64_t tick_advance_multiple;
    rt_tick_t tick_advance[RTPROBE_RECORDED_INTERVALS];
    int recorded_misses;
    int missed_sample[RTPROBE_RECORDED_MISSES];
    uint64_t missed_actual_ns[RTPROBE_RECORDED_MISSES];
};

enum rtprobe_mode
{
    RTPROBE_CONTINUOUS,
    RTPROBE_PERIODIC_THREAD,
};

struct rtprobe_job
{
    enum rtprobe_mode mode;
    uint64_t period;
    int samples;
    uint64_t work_us;
    struct rtprobe_result result;
    int status;
    struct rt_semaphore done;
};

struct rtprobe_stress_job
{
    int worker;
    volatile int stop;
    uint64_t iterations;
    uint32_t checksum;
    int started;
    struct rt_semaphore done;
};

static inline uint64_t cntvct(void)
{
    uint64_t value;
    __asm__ volatile("mrs %0, cntvct_el0" : "=r"(value));
    return value;
}

static inline uint64_t cntfrq(void)
{
    uint64_t value;
    __asm__ volatile("mrs %0, cntfrq_el0" : "=r"(value));
    return value;
}

static uint64_t abs_diff_u64(uint64_t left, uint64_t right)
{
    return left > right ? left - right : right - left;
}

static uint64_t cycles_to_ns(uint64_t cycles, uint64_t frequency)
{
    return cycles * 1000000000ULL / frequency;
}

static void sort_u64(uint64_t *values, int count)
{
    int gap;

    for (gap = count / 2; gap > 0; gap /= 2)
    {
        int index;

        for (index = gap; index < count; index++)
        {
            uint64_t value = values[index];
            int cursor = index;

            while (cursor >= gap && values[cursor - gap] > value)
            {
                values[cursor] = values[cursor - gap];
                cursor -= gap;
            }
            values[cursor] = value;
        }
    }
}

static int percentile_index(int count, int permille)
{
    int index = (int)(((uint64_t)count * permille + 999) / 1000);

    if (index <= 0)
    {
        return 0;
    }
    if (index >= count)
    {
        return count - 1;
    }
    return index - 1;
}

static void print_u64(uint64_t value)
{
    char buffer[32];
    int index = 30;

    buffer[31] = '\0';
    if (value == 0)
    {
        rt_kprintf("0");
        return;
    }

    while (value != 0 && index >= 0)
    {
        buffer[index--] = '0' + (char)(value % 10);
        value /= 10;
    }
    rt_kprintf("%s", &buffer[index + 1]);
}

static void init_result(struct rtprobe_result *result, int samples)
{
    result->samples = samples;
    result->missed = 0;
    result->sum = 0;
    result->max = 0;
    result->p99 = 0;
    result->p999 = 0;
    result->actual_min = UINT64_MAX;
    result->actual_max = 0;
    result->below_half_period = 0;
    result->above_three_halves = 0;
    result->recorded_intervals = 0;
    result->tracks_tick_advance = 0;
    result->tick_advance_zero = 0;
    result->tick_advance_multiple = 0;
    result->recorded_misses = 0;
}

static void record_interval(struct rtprobe_result *result, uint64_t actual_ns,
                            uint64_t target_ns)
{
    if (actual_ns < result->actual_min)
    {
        result->actual_min = actual_ns;
    }
    if (actual_ns > result->actual_max)
    {
        result->actual_max = actual_ns;
    }
    if (actual_ns < target_ns / 2)
    {
        result->below_half_period++;
    }
    if (actual_ns > target_ns + target_ns / 2)
    {
        result->above_three_halves++;
    }
    if (result->recorded_intervals < RTPROBE_RECORDED_INTERVALS)
    {
        result->actual_interval_ns[result->recorded_intervals++] = actual_ns;
    }
}

static void record_tick_advance(struct rtprobe_result *result, int sample,
                                rt_tick_t advance)
{
    if (advance == 0)
    {
        result->tick_advance_zero++;
    }
    if (advance > 1)
    {
        result->tick_advance_multiple++;
    }
    if (sample < RTPROBE_RECORDED_INTERVALS)
    {
        result->tick_advance[sample] = advance;
    }
}

static int run_rtprobe(uint64_t period_us, int samples, uint64_t work_us,
                       struct rtprobe_result *result)
{
    uint64_t frequency;
    uint64_t period_cycles;
    uint64_t target_ns;
    uint64_t previous;
    uint64_t deadline;
    int index;

    if (period_us == 0 || samples <= 0 || samples > RTPROBE_MAX_SAMPLES)
    {
        return -1;
    }

    frequency = cntfrq();
    period_cycles = frequency * period_us / 1000000ULL;
    target_ns = period_us * 1000ULL;
    init_result(result, samples);
    previous = cntvct();
    deadline = previous;

    for (index = 0; index < samples; index++)
    {
        uint64_t now;
        uint64_t actual_ns;
        uint64_t jitter;

        deadline += period_cycles;
        while ((int64_t)(cntvct() - deadline) < 0)
        {
        }
        now = cntvct();
        if (work_us != 0)
        {
            rt_hw_us_delay((rt_uint32_t)work_us);
        }

        actual_ns = cycles_to_ns(now - previous, frequency);
        jitter = abs_diff_u64(actual_ns, target_ns);
        previous = now;
        record_interval(result, actual_ns, target_ns);
        jitter_ns[index] = jitter;
        result->sum += jitter;
        if (jitter > result->max)
        {
            result->max = jitter;
        }
        if (actual_ns > target_ns * 2)
        {
            if (result->recorded_misses < RTPROBE_RECORDED_MISSES)
            {
                int recorded = result->recorded_misses++;

                result->missed_sample[recorded] = index;
                result->missed_actual_ns[recorded] = actual_ns;
            }
            result->missed++;
        }
    }

    sort_u64(jitter_ns, samples);
    result->p99 = jitter_ns[percentile_index(samples, 990)];
    result->p999 = jitter_ns[percentile_index(samples, 999)];
    return 0;
}

static int run_periodic_thread_probe(rt_tick_t period_ticks, int samples,
                                     struct rtprobe_result *result)
{
    uint64_t frequency;
    uint64_t target_cycles;
    uint64_t previous;
    rt_tick_t previous_tick;
    rt_tick_t wake_tick;
    int index;

    if (period_ticks == 0 || samples <= 0 || samples > RTPROBE_MAX_SAMPLES)
    {
        return -1;
    }

    frequency = cntfrq();
    target_cycles = frequency * period_ticks / RT_TICK_PER_SECOND;
    if (target_cycles == 0)
    {
        return -1;
    }
    init_result(result, samples);
    result->tracks_tick_advance = 1;
    wake_tick = rt_tick_get();

    /* Align the first measured interval to a real RT-Thread tick wakeup. */
    if (rt_thread_delay_until(&wake_tick, period_ticks) != RT_EOK)
    {
        return -1;
    }
    previous = cntvct();
    previous_tick = rt_tick_get();

    for (index = 0; index < samples; index++)
    {
        uint64_t now;
        uint64_t actual_ns;
        uint64_t target_ns;
        uint64_t jitter;
        rt_tick_t current_tick;

        if (rt_thread_delay_until(&wake_tick, period_ticks) != RT_EOK)
        {
            return -1;
        }
        now = cntvct();
        current_tick = rt_tick_get();
        record_tick_advance(result, index, current_tick - previous_tick);
        previous_tick = current_tick;
        actual_ns = cycles_to_ns(now - previous, frequency);
        target_ns = cycles_to_ns(target_cycles, frequency);
        jitter = abs_diff_u64(actual_ns, target_ns);
        previous = now;
        record_interval(result, actual_ns, target_ns);
        jitter_ns[index] = jitter;
        result->sum += jitter;
        if (jitter > result->max)
        {
            result->max = jitter;
        }
        if (actual_ns > target_ns * 2)
        {
            if (result->recorded_misses < RTPROBE_RECORDED_MISSES)
            {
                int recorded = result->recorded_misses++;

                result->missed_sample[recorded] = index;
                result->missed_actual_ns[recorded] = actual_ns;
            }
            result->missed++;
        }
    }

    sort_u64(jitter_ns, samples);
    result->p99 = jitter_ns[percentile_index(samples, 990)];
    result->p999 = jitter_ns[percentile_index(samples, 999)];
    return 0;
}

static void rtprobe_worker(void *parameter)
{
    struct rtprobe_job *job = parameter;

    if (job->mode == RTPROBE_CONTINUOUS)
    {
        job->status = run_rtprobe(job->period, job->samples, job->work_us,
                                  &job->result);
    }
    else
    {
        job->status = run_periodic_thread_probe((rt_tick_t)job->period,
                                                job->samples, &job->result);
    }
    rt_sem_release(&job->done);
}

static int run_worker_job(struct rtprobe_job *job)
{
    rt_thread_t worker;

    job->status = -1;
    if (rt_sem_init(&job->done, "rtdone", 0, RT_IPC_FLAG_PRIO) != RT_EOK)
    {
        return -1;
    }
    worker = rt_thread_create("rtprobe", rtprobe_worker, job,
                              RTPROBE_STACK_SIZE, RTPROBE_PRIORITY, 1);
    if (worker == RT_NULL)
    {
        rt_sem_detach(&job->done);
        return -1;
    }
    if (rt_thread_startup(worker) != RT_EOK ||
        rt_sem_take(&job->done, RT_WAITING_FOREVER) != RT_EOK)
    {
        rt_sem_detach(&job->done);
        return -1;
    }
    rt_sem_detach(&job->done);
    return job->status;
}

static void rtprobe_stress_worker(void *parameter)
{
    struct rtprobe_stress_job *job = parameter;
    uint32_t state = 0x9e3779b9U ^ (uint32_t)job->worker;

    while (!job->stop)
    {
        int index;

        for (index = 0; index < RTPROBE_STRESS_WORDS; index += 16)
        {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            stress_words[job->worker][index] ^= state + (uint32_t)index;
        }
        job->checksum ^= state;
        job->iterations++;
    }
    rt_sem_release(&job->done);
}

static void stop_stress_workers(struct rtprobe_stress_job *jobs)
{
    int index;

    for (index = 0; index < RTPROBE_STRESS_WORKERS; index++)
    {
        if (jobs[index].started)
        {
            jobs[index].stop = 1;
        }
    }
    for (index = 0; index < RTPROBE_STRESS_WORKERS; index++)
    {
        if (jobs[index].started)
        {
            rt_sem_take(&jobs[index].done, RT_WAITING_FOREVER);
        }
        rt_sem_detach(&jobs[index].done);
    }
}

static int start_stress_workers(struct rtprobe_stress_job *jobs)
{
    static const char *names[RTPROBE_STRESS_WORKERS] = {"rtload0", "rtload1"};
    int index;

    for (index = 0; index < RTPROBE_STRESS_WORKERS; index++)
    {
        jobs[index].worker = index;
        jobs[index].stop = 0;
        jobs[index].iterations = 0;
        jobs[index].checksum = 0;
        jobs[index].started = 0;
        if (rt_sem_init(&jobs[index].done, names[index], 0, RT_IPC_FLAG_PRIO) != RT_EOK)
        {
            while (index-- > 0)
            {
                rt_sem_detach(&jobs[index].done);
            }
            return -1;
        }
    }
    for (index = 0; index < RTPROBE_STRESS_WORKERS; index++)
    {
        rt_thread_t worker = rt_thread_create(
            names[index], rtprobe_stress_worker, &jobs[index], RTPROBE_STACK_SIZE,
            RTPROBE_STRESS_PRIORITY, 1);

        if (worker == RT_NULL || rt_thread_startup(worker) != RT_EOK)
        {
            stop_stress_workers(jobs);
            return -1;
        }
        jobs[index].started = 1;
    }
    return 0;
}

static void print_result(const char *name, const struct rtprobe_result *result)
{
    rt_kprintf("%s samples=%d missed=", name, result->samples);
    print_u64(result->missed);
    rt_kprintf("\njitter_ns avg=");
    print_u64(result->sum / (uint64_t)result->samples);
    rt_kprintf(" max=");
    print_u64(result->max);
    rt_kprintf(" p99=");
    print_u64(result->p99);
    rt_kprintf(" p999=");
    print_u64(result->p999);
    rt_kprintf("\n");
    rt_kprintf("actual_interval_ns min=");
    print_u64(result->actual_min);
    rt_kprintf(" max=");
    print_u64(result->actual_max);
    rt_kprintf(" below_half_period=");
    print_u64(result->below_half_period);
    rt_kprintf(" above_three_halves=");
    print_u64(result->above_three_halves);
    if (result->tracks_tick_advance)
    {
        rt_kprintf(" tick_advance_zero=");
        print_u64(result->tick_advance_zero);
        rt_kprintf(" tick_advance_multiple=");
        print_u64(result->tick_advance_multiple);
    }
    rt_kprintf("\nfirst_intervals_ns");
    for (int index = 0; index < result->recorded_intervals; index++)
    {
        rt_kprintf(" sample=%d actual_ns=", index);
        print_u64(result->actual_interval_ns[index]);
        if (result->tracks_tick_advance)
        {
            rt_kprintf(" tick_advance=");
            print_u64(result->tick_advance[index]);
        }
    }
    rt_kprintf("\n");
    if (result->recorded_misses != 0)
    {
        int index;

        rt_kprintf("missed_intervals");
        for (index = 0; index < result->recorded_misses; index++)
        {
            rt_kprintf(" sample=%d actual_ns=", result->missed_sample[index]);
            print_u64(result->missed_actual_ns[index]);
        }
        if (result->missed > (uint64_t)result->recorded_misses)
        {
            rt_kprintf(" truncated=1");
        }
        rt_kprintf("\n");
    }
}

static void rtprobe(int argc, char **argv)
{
    struct rtprobe_job job;

    if (argc != 4)
    {
        rt_kprintf("usage: rtprobe <period_us> <samples> <work_us>\n");
        return;
    }
    job.mode = RTPROBE_CONTINUOUS;
    job.period = strtoull(argv[1], RT_NULL, 0);
    job.samples = atoi(argv[2]);
    job.work_us = strtoull(argv[3], RT_NULL, 0);
    if (run_worker_job(&job) != 0)
    {
        rt_kprintf("rtprobe failed: invalid arguments or worker error\n");
        return;
    }
    print_result("continuous", &job.result);
}
MSH_CMD_EXPORT(rtprobe, RT-Thread high-priority vCPU residency probe);

static const char *rtprobe_platform(void)
{
    const char *platform = rt_ofw_bootargs_select("rtprobe.platform=", 0);

    return platform && platform[0] ? platform : "unknown";
}

static int periodic_result_passed(const struct rtprobe_result *result)
{
    return result->missed == 0 && result->below_half_period == 0 &&
           result->above_three_halves == 0 && result->tick_advance_zero == 0 &&
           result->tick_advance_multiple == 0;
}

static int run_rtprobe_smoke(const char *platform)
{
    struct rtprobe_job continuous;
    struct rtprobe_job periodic;
    int require_vcpu_residency = rt_strcmp(platform, "native") != 0;

    continuous.mode = RTPROBE_CONTINUOUS;
    continuous.period = 1000;
    continuous.samples = 2000;
    continuous.work_us = 0;
    if (run_worker_job(&continuous) != 0)
    {
        rt_kprintf("rtprobe smoke failed: continuous worker error\n");
        return -1;
    }
    print_result("continuous_1ms", &continuous.result);

    periodic.mode = RTPROBE_PERIODIC_THREAD;
    periodic.period = 1;
    periodic.samples = 200;
    periodic.work_us = 0;
    if (run_worker_job(&periodic) != 0)
    {
        rt_kprintf("rtprobe smoke failed: periodic worker error\n");
        return -1;
    }
    print_result("periodic_thread_10ms", &periodic.result);

    if ((!require_vcpu_residency || continuous.result.missed == 0) &&
        periodic_result_passed(&periodic.result))
    {
        rt_kprintf("RTTHREAD_RTPROBE_SMOKE_PASSED platform=%s\n", platform);
        return 0;
    }
    else
    {
        rt_kprintf("rtprobe smoke failed: continuous_missed=");
        print_u64(continuous.result.missed);
        rt_kprintf(" periodic_missed=");
        print_u64(periodic.result.missed);
        rt_kprintf(" periodic_below_half=");
        print_u64(periodic.result.below_half_period);
        rt_kprintf(" periodic_above_three_halves=");
        print_u64(periodic.result.above_three_halves);
        rt_kprintf(" tick_advance_zero=");
        print_u64(periodic.result.tick_advance_zero);
        rt_kprintf(" tick_advance_multiple=");
        print_u64(periodic.result.tick_advance_multiple);
        rt_kprintf(" platform=%s\n", platform);
        return -1;
    }
}

static void rtprobe_smoke(void)
{
    run_rtprobe_smoke(rtprobe_platform());
}
MSH_CMD_EXPORT(rtprobe_smoke, AxVisor RT-Thread timer and scheduling smoke test);

static int run_rtprobe_closure(const char *platform)
{
    struct rtprobe_job idle;
    struct rtprobe_job stress;
    struct rtprobe_stress_job stress_jobs[RTPROBE_STRESS_WORKERS];
    int stress_ok = 1;
    int index;

    rt_kprintf("RTPROBE_SCENARIO_STARTED name=idle period_ms=10 samples=%d\n",
               RTPROBE_IDLE_SAMPLES);
    idle.mode = RTPROBE_PERIODIC_THREAD;
    idle.period = 1;
    idle.samples = RTPROBE_IDLE_SAMPLES;
    idle.work_us = 0;
    if (run_worker_job(&idle) != 0)
    {
        rt_kprintf("rtprobe closure failed: idle worker error platform=%s\n", platform);
        return -1;
    }
    print_result("periodic_idle_10ms", &idle.result);
    if (!periodic_result_passed(&idle.result))
    {
        rt_kprintf("rtprobe closure failed: idle timing gate platform=%s\n", platform);
        return -1;
    }
    rt_kprintf("RTPROBE_SCENARIO_PASSED name=idle platform=%s\n", platform);

    if (start_stress_workers(stress_jobs) != 0)
    {
        rt_kprintf("rtprobe closure failed: stress worker startup platform=%s\n", platform);
        return -1;
    }
    rt_kprintf("RTPROBE_SCENARIO_STARTED name=stress-long period_ms=10 samples=%d workers=%d\n",
               RTPROBE_LONG_STRESS_SAMPLES, RTPROBE_STRESS_WORKERS);
    stress.mode = RTPROBE_PERIODIC_THREAD;
    stress.period = 1;
    stress.samples = RTPROBE_LONG_STRESS_SAMPLES;
    stress.work_us = 0;
    if (run_worker_job(&stress) != 0)
    {
        stop_stress_workers(stress_jobs);
        rt_kprintf("rtprobe closure failed: stress timing worker platform=%s\n", platform);
        return -1;
    }
    stop_stress_workers(stress_jobs);
    print_result("periodic_stress_long_10ms", &stress.result);
    rt_kprintf("stress_load");
    for (index = 0; index < RTPROBE_STRESS_WORKERS; index++)
    {
        rt_kprintf(" worker=%d iterations=", index);
        print_u64(stress_jobs[index].iterations);
        rt_kprintf(" checksum=%u", stress_jobs[index].checksum);
        if (stress_jobs[index].iterations == 0)
        {
            stress_ok = 0;
        }
    }
    rt_kprintf("\n");
    if (!stress_ok || !periodic_result_passed(&stress.result))
    {
        rt_kprintf("rtprobe closure failed: stress timing gate platform=%s\n", platform);
        return -1;
    }

    rt_kprintf("rtprobe_compare idle_max_ns=");
    print_u64(idle.result.max);
    rt_kprintf(" idle_p99_ns=");
    print_u64(idle.result.p99);
    rt_kprintf(" stress_max_ns=");
    print_u64(stress.result.max);
    rt_kprintf(" stress_p99_ns=");
    print_u64(stress.result.p99);
    rt_kprintf("\n");
    rt_kprintf("RTPROBE_SCENARIO_PASSED name=stress-long platform=%s\n", platform);
    rt_kprintf("RTTHREAD_RTPROBE_CLOSURE_PASSED platform=%s\n", platform);
    return 0;
}

static void rtprobe_autorun_entry(void *parameter)
{
    const char *platform;
    const char *profile;

    (void)parameter;

    /* Let application initialization and the shell finish before measuring. */
    rt_thread_mdelay(100);
    platform = rtprobe_platform();
    profile = rt_ofw_bootargs_select("rtprobe.profile=", 0);
    rt_kprintf("RTTHREAD_RTPROBE_STARTED platform=%s profile=%s\n", platform,
               profile && profile[0] ? profile : "smoke");
    if (run_rtprobe_smoke(platform) == 0 && profile && !rt_strcmp(profile, "closure"))
    {
        run_rtprobe_closure(platform);
    }
}

static int rtprobe_autorun(void)
{
    rt_thread_t thread = rt_thread_create(
        "rtptest", rtprobe_autorun_entry, RT_NULL, RTPROBE_STACK_SIZE,
        RTPROBE_AUTORUN_PRIORITY, 1);

    if (thread == RT_NULL || rt_thread_startup(thread) != RT_EOK)
    {
        rt_kprintf("rtprobe smoke failed: autorun thread error\n");
        return -1;
    }
    return 0;
}
INIT_APP_EXPORT(rtprobe_autorun);
