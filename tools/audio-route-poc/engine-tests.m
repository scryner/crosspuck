// Pure engine regression tests: no HAL objects, permission prompts or audio IO.
#include "../../crates/crosspuck-app/src/audio/router.m"
#include <assert.h>

static void *notify_output_later(void *unused) {
    (void)unused;
    usleep(50000);
    output_changed(0, 0, NULL, NULL);
    return NULL;
}

static void test_control_wait(void) {
    assert(setup_control_wait());
    atomic_store(&stopping, false);
    double start = monotonic_seconds();
    pump(0.15);
    // An empty run loop must actually sleep, not return Finished immediately.
    assert(monotonic_seconds() - start >= 0.12);
    pthread_t notifier;
    uint64_t generation = atomic_load(&output_changes);
    assert(!pthread_create(&notifier, NULL, notify_output_later, NULL));
    start = monotonic_seconds();
    pump(2);
    assert(monotonic_seconds() - start < 1);
    pthread_join(notifier, NULL);
    assert(atomic_load(&output_changes) == generation + 1);
    // Notification before entering the wait must also be retained.
    wake_control();
    start = monotonic_seconds();
    pump(2);
    assert(monotonic_seconds() - start < 1);
    teardown_control_wait();
}

static void test_parent_watch(void) {
    int saved_stdin = dup(STDIN_FILENO);
    assert(saved_stdin >= 0);
    for (int mode = 0; mode < 3; ++mode) {
        int owner_pipe[2];
        assert(!pipe(owner_pipe));
        assert(dup2(owner_pipe[0], STDIN_FILENO) >= 0);
        close(owner_pipe[0]);
        atomic_store(&stopping, false);
        atomic_store(&finished, false);
        assert(setup_control_wait());
        pthread_t watcher;
        assert(start_parent_watch(&watcher));
        double start = monotonic_seconds();
        if (mode == 0) {
            close(owner_pipe[1]); // Abrupt owner exit: EOF wakes main run loop.
            pump(2);
            assert(atomic_load(&stopping));
        } else if (mode == 1) {
            interrupt_handler(SIGTERM); // Signal must wake a blocking poll.
        }
        // mode 2 exercises clean exit with an owner that still holds stdin.
        finish_parent_watch(watcher);
        assert(monotonic_seconds() - start < 1);
        assert(shutdown_pipe[0] == -1 && shutdown_pipe[1] == -1);
        teardown_control_wait();
        if (mode != 0) close(owner_pipe[1]);
    }
    assert(dup2(saved_stdin, STDIN_FILENO) >= 0);
    close(saved_stdin);
    atomic_store(&stopping, false);
}

int main(void) {
    @autoreleasepool {
        test_control_wait();
        test_parent_watch();
        int argc = 2;
        const char args[] = "/path/wine\0\0game.exe\0--play\0\0\0UNRELATED=test\0WINEPREFIX=/bottles/Test\0SECRET=not-retained\0";
        NSMutableData *bytes = [NSMutableData dataWithBytes:&argc length:sizeof(argc)];
        [bytes appendBytes:args length:sizeof(args)];
        NSDictionary *identity = wine_arguments(bytes);
        assert([identity[@"command"] isEqual:@"game.exe"]);
        assert([identity[@"bottle_path"] isEqual:@"/bottles/Test"]);
        assert(identity.count == 2);
        assert(wine_arguments([NSData dataWithBytes:args length:2]) == nil);
        const char truncated[] = "/wine\0game.exe\0--play\0WINEPREFIX=/no-terminator";
        bytes = [NSMutableData dataWithBytes:&argc length:sizeof(argc)];
        [bytes appendBytes:truncated length:sizeof(truncated)-1];
        assert(wine_arguments(bytes) == nil);

        NSArray *processes = @[
            @{@"audio_object":@101, @"running_output":@YES, @"bottle_path":@"/bottles/A", @"devices":@[@{@"id":@1}]},
            @{@"audio_object":@102, @"running_output":@YES, @"bottle_path":@"/external/B", @"devices":@[@{@"id":@1}]},
            @{@"audio_object":@103, @"running_output":@YES, @"devices":@[@{@"id":@2}]},
            @{@"audio_object":@104, @"running_output":@NO, @"devices":@[@{@"id":@1}]},
            @{@"audio_object":@105, @"running_output":@YES, @"devices":@[]},
        ];
        assert([routing_targets(processes, 2) isEqualToArray:(@[@101, @102])]);
        assert([routing_targets(processes, 1) isEqualToArray:@[@103]]);
        assert(routing_targets(@[], 2).count == 0);

        AudioStreamBasicDescription format = { .mSampleRate=48000, .mFormatID=kAudioFormatLinearPCM,
            .mFormatFlags=kAudioFormatFlagsNativeFloatPacked, .mBitsPerChannel=32,
            .mChannelsPerFrame=2, .mBytesPerFrame=8 };
        assert(float_format(format));
        format.mSampleRate = NAN; assert(!float_format(format));
        format.mSampleRate = 44100; assert(float_format(format));
        format.mFormatFlags |= kAudioFormatFlagIsBigEndian; assert(!float_format(format));
        format.mFormatFlags = kAudioFormatFlagIsFloat; assert(!float_format(format));

        float samples[] = {0.25f, -0.5f, 0, 0, NAN, 0.2f, 2, -2};
        float destination[32];
        AudioBufferList input = {1, {{2, sizeof(samples), samples}}};
        AudioBufferList output = {1, {{8, sizeof(destination), destination}}};
        AudioContext ctx = {.sample_rate=48000, .gain=1, .output_left_channel=3, .output_right_channel=8};
        atomic_init(&ctx.frames, 0); atomic_init(&ctx.bad_buffers, 0);
        audio_io(0, NULL, &input, NULL, &output, NULL, &ctx);
        assert(atomic_load(&ctx.frames) == 4);
        assert(atomic_load(&ctx.bad_buffers) == 0);
        assert(destination[2] == 0.25f && destination[7] == -0.5f);
        assert(destination[26] == 1 && destination[31] == -1);
        for (unsigned i=0; i<32; ++i)
            if (i%8 != 2 && i%8 != 7) assert(destination[i] == 0);
        assert(destination[18] == 0 && destination[23] == 0);

        memset(samples, 0, sizeof(samples));
        audio_io(0, NULL, &input, NULL, &output, NULL, &ctx);
        assert(atomic_load(&ctx.frames) == 8); // Digital silence is healthy.
        for (unsigned i=0; i<32; ++i) assert(destination[i] == 0);
        output.mBuffers[0].mDataByteSize -= 8*sizeof(float);
        audio_io(0, NULL, &input, NULL, &output, NULL, &ctx);
        assert(atomic_load(&ctx.bad_buffers) == 1);
        assert(atomic_load(&ctx.frames) == 8);

        struct { UInt32 count; AudioBuffer buffers[2]; } duplex = {2, {{1, 4, samples}, {2, sizeof(samples), samples}}};
        float *left, *right; UInt32 stride, frames;
        assert(!stereo_view((AudioBufferList *)&duplex, &left, &right, &stride, &frames, 1));
        duplex.buffers[0].mData = NULL;
        assert(stereo_view((AudioBufferList *)&duplex, &left, &right, &stride, &frames, 1));
        assert(frames == 4);
        puts("Audio engine regression tests passed");
    }
}
