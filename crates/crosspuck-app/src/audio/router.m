// CrossOver-only routing engine. Runs in the supervised CrossPuckAudio helper.
#import <Foundation/Foundation.h>
#import <CoreAudio/CoreAudio.h>
#import <CoreAudio/AudioHardwareTapping.h>
#import <CoreAudio/CATapDescription.h>
#include <libproc.h>
#include <sys/sysctl.h>
#include <signal.h>
#include <stdatomic.h>
#include <mach/mach_time.h>
#include <math.h>
#include <unistd.h>
#include <pthread.h>
#include <poll.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <fcntl.h>
#include <errno.h>

static _Atomic bool stopping;
static _Atomic uint64_t output_changes;
static _Atomic bool finished;
static int shutdown_pipe[2] = {-1, -1};
static pthread_mutex_t control_lock = PTHREAD_MUTEX_INITIALIZER;
static CFRunLoopRef control_loop;
static CFRunLoopSourceRef control_source;

static void notify_shutdown(void) {
    int saved_errno = errno;
    // Nonblocking and async-signal-safe; a full pipe is already a notification.
    char byte = 1;
    if (shutdown_pipe[1] >= 0) (void)write(shutdown_pipe[1], &byte, 1);
    errno = saved_errno;
}

static void interrupt_handler(int sig) {
    (void)sig;
    stopping = true;
    notify_shutdown();
}

static void wake_control(void) {
    // Only control notifications take this lock, never the audio IO callback.
    pthread_mutex_lock(&control_lock);
    if (control_source) {
        CFRunLoopSourceSignal(control_source);
        CFRunLoopWakeUp(control_loop);
    }
    pthread_mutex_unlock(&control_lock);
}

static void control_perform(void *unused) { (void)unused; }

static BOOL setup_control_wait(void) {
    CFRunLoopSourceContext context = {.perform = control_perform};
    control_source = CFRunLoopSourceCreate(NULL, 0, &context);
    if (!control_source) return NO;
    control_loop = CFRunLoopGetCurrent();
    CFRunLoopAddSource(control_loop, control_source, kCFRunLoopDefaultMode);
    return YES;
}

static void teardown_control_wait(void) {
    pthread_mutex_lock(&control_lock);
    if (control_source) {
        CFRunLoopRemoveSource(control_loop, control_source, kCFRunLoopDefaultMode);
        CFRelease(control_source);
        control_source = NULL;
        control_loop = NULL;
    }
    pthread_mutex_unlock(&control_lock);
}

static void pump(double seconds) {
    // A registered source keeps an otherwise empty run loop from returning
    // immediately. Output changes and parent exit interrupt this timed wait.
    if (seconds > 0 && !stopping) CFRunLoopRunInMode(kCFRunLoopDefaultMode, seconds, true);
}

// EOF on the private stdin pipe means the owner quit or crashed. A stuck HAL
// call cannot keep a muted game alive indefinitely: process exit releases the
// private tap even if graceful cleanup does not return.
static void *watch_parent(void *unused) {
    (void)unused;
    struct pollfd pipes[] = {{STDIN_FILENO, POLLIN | POLLHUP, 0},
                             {shutdown_pipe[0], POLLIN | POLLHUP, 0}};
    while (!stopping) {
        int ready = poll(pipes, 2, -1);
        if (ready < 0 && errno != EINTR) break;
        if (ready > 0) {
            char byte;
            if (pipes[1].revents) break;
            if (pipes[0].revents & (POLLERR | POLLNVAL)) break;
            if ((pipes[0].revents & (POLLIN | POLLHUP)) && read(STDIN_FILENO, &byte, 1) <= 0) break;
        }
    }
    stopping = 1;
    wake_control();
    for (int i = 0; i < 30 && !atomic_load(&finished); ++i) usleep(100000);
    if (!atomic_load(&finished)) _exit(10);
    return NULL;
}

static double monotonic_seconds(void) {
    mach_timebase_info_data_t clock;
    mach_timebase_info(&clock);
    return mach_absolute_time() * (double)clock.numer / clock.denom / 1e9;
}

static OSStatus output_changed(AudioObjectID object, UInt32 count,
                                const AudioObjectPropertyAddress *properties, void *context) {
    (void)object; (void)count; (void)properties; (void)context;
    atomic_fetch_add(&output_changes, 1);
    wake_control();
    return noErr;
}

static AudioObjectPropertyAddress address(AudioObjectPropertySelector selector,
                                          AudioObjectPropertyScope scope) {
    return (AudioObjectPropertyAddress){selector, scope, kAudioObjectPropertyElementMain};
}

static NSData *property(AudioObjectID object, AudioObjectPropertySelector selector,
                        AudioObjectPropertyScope scope) {
    AudioObjectPropertyAddress a = address(selector, scope);
    UInt32 size = 0;
    if (AudioObjectGetPropertyDataSize(object, &a, 0, NULL, &size) != noErr) return nil;
    NSMutableData *data = [NSMutableData dataWithLength:size];
    if (AudioObjectGetPropertyData(object, &a, 0, NULL, &size, data.mutableBytes) != noErr) return nil;
    data.length = size;
    return data;
}

static UInt32 integer(AudioObjectID object, AudioObjectPropertySelector selector,
                       AudioObjectPropertyScope scope) {
    UInt32 value = 0;
    UInt32 size = sizeof(value);
    AudioObjectPropertyAddress a = address(selector, scope);
    if (AudioObjectGetPropertyData(object, &a, 0, NULL, &size, &value) != noErr || size != sizeof(value)) return 0;
    return value;
}

static double sample_rate(AudioObjectID object) {
    double rate = 0;
    UInt32 size = sizeof(rate);
    AudioObjectPropertyAddress a = address(kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal);
    if (AudioObjectGetPropertyData(object, &a, 0, NULL, &size, &rate) != noErr || size != sizeof(rate)) return 0;
    return rate;
}

static NSString *string_property(AudioObjectID object, AudioObjectPropertySelector selector) {
    CFStringRef value = NULL;
    UInt32 size = sizeof(value);
    AudioObjectPropertyAddress a = address(selector, kAudioObjectPropertyScopeGlobal);
    if (AudioObjectGetPropertyData(object, &a, 0, NULL, &size, &value) != noErr) return nil;
    return CFBridgingRelease(value);
}

static NSArray<NSNumber *> *object_list(AudioObjectID object,
                                        AudioObjectPropertySelector selector,
                                        AudioObjectPropertyScope scope) {
    NSData *data = property(object, selector, scope);
    const AudioObjectID *ids = data.bytes;
    NSMutableArray *result = [NSMutableArray array];
    for (NSUInteger i = 0; i < data.length / sizeof(*ids); ++i) [result addObject:@(ids[i])];
    return result;
}

static void event(NSDictionary *record) {
    NSMutableDictionary *json = [record mutableCopy];
    json[@"time"] = @([[NSDate date] timeIntervalSince1970]);
    NSData *data = [NSJSONSerialization dataWithJSONObject:json options:NSJSONWritingSortedKeys error:NULL];
    if (data) { fwrite(data.bytes, 1, data.length, stdout); fputc('\n', stdout); fflush(stdout); }
}

static BOOL checked(OSStatus status, const char *operation) {
    if (status == noErr) return YES;
    event(@{@"event":@"error", @"operation":@(operation), @"status":@(status)});
    return NO;
}

// Only argv[0] and WINEPREFIX leave this parser; unrelated environment values
// are never retained in diagnostics or sent to the parent.
static NSDictionary *wine_arguments(NSData *data) {
    size_t size = data.length;
    if (size < sizeof(int)) return nil;
    const char *start = data.bytes, *end = start + size, *cursor = start + sizeof(int);
    int argc = 0; memcpy(&argc, start, sizeof(argc));
    if (argc < 1 || argc > 4096) return nil;
    size_t len = strnlen(cursor, (size_t)(end - cursor));
    cursor += len;
    while (cursor < end && !*cursor) ++cursor;
    NSString *command = nil;
    for (int i = 0; i < argc && cursor < end; ++i) {
        len = strnlen(cursor, (size_t)(end - cursor));
        if (cursor + len == end) return nil;
        if (i == 0) command = [[NSString alloc] initWithBytes:cursor length:len encoding:NSUTF8StringEncoding];
        cursor += len + 1;
    }
    NSString *prefix = nil;
    while (cursor < end) {
        // KERN_PROCARGS2 can pad the argv/environment boundary with NULs.
        while (cursor < end && !*cursor) ++cursor;
        if (cursor == end) break;
        len = strnlen(cursor, (size_t)(end - cursor));
        if (cursor + len == end) break;
        if (len > 11 && !memcmp(cursor, "WINEPREFIX=", 11)) {
            prefix = [[NSString alloc] initWithBytes:cursor + 11 length:len - 11 encoding:NSUTF8StringEncoding];
            break;
        }
        cursor += len + 1;
    }
    if (!prefix.length) return nil;
    return @{@"command":command ?: @"", @"bottle_path":prefix};
}

static NSDictionary *wine_identity(pid_t pid) {
    int mib[] = {CTL_KERN, KERN_PROCARGS2, pid};
    size_t size = 0;
    if (sysctl(mib, 3, NULL, &size, NULL, 0) != 0 || size > 1024 * 1024) return nil;
    NSMutableData *data = [NSMutableData dataWithLength:size];
    if (sysctl(mib, 3, data.mutableBytes, &size, NULL, 0) != 0) return nil;
    data.length = size;
    NSDictionary *args = wine_arguments(data);
    if (!args) return nil;
    NSString *prefix = [args[@"bottle_path"] stringByResolvingSymlinksInPath].stringByStandardizingPath;
    if (![[NSFileManager defaultManager] fileExistsAtPath:[prefix stringByAppendingPathComponent:@"cxbottle.conf"]]) return nil;
    return @{@"command":args[@"command"], @"bottle_path":prefix};
}

static NSDictionary *device_info(AudioDeviceID device) {
    return @{@"id":@(device), @"name":string_property(device, kAudioObjectPropertyName) ?: @"",
             @"uid":string_property(device, kAudioDevicePropertyDeviceUID) ?: @"",
             @"sample_rate":@(sample_rate(device)),
             @"input_streams":object_list(device, kAudioDevicePropertyStreams, kAudioObjectPropertyScopeInput),
             @"output_streams":object_list(device, kAudioDevicePropertyStreams, kAudioObjectPropertyScopeOutput)};
}

// Detailed inventory is only used by --list. The recurring scan skips idle
// clients and reads no device names, UIDs, formats or process environment for
// clients already playing through the current default output.
static NSArray<NSDictionary *> *wine_processes(BOOL inventory, AudioDeviceID output) {
    NSMutableArray *result = [NSMutableArray array];
    for (NSNumber *object in object_list(kAudioObjectSystemObject, kAudioHardwarePropertyProcessObjectList, kAudioObjectPropertyScopeGlobal)) {
        AudioObjectID oid = object.unsignedIntValue;
        BOOL running = integer(oid, kAudioProcessPropertyIsRunningOutput, kAudioObjectPropertyScopeGlobal) != 0;
        if (!inventory && !running) continue;
        NSString *bundle = string_property(oid, kAudioProcessPropertyBundleID);
        if (![bundle hasPrefix:@"com.codeweavers.CrossOver."]) continue;
        pid_t pid = (pid_t)integer(oid, kAudioProcessPropertyPID, kAudioObjectPropertyScopeGlobal);
        if (pid == getpid()) continue;
        NSArray<NSNumber *> *device_ids = object_list(oid, kAudioProcessPropertyDevices, kAudioObjectPropertyScopeOutput);
        if (!inventory) {
            BOOL mismatched = NO;
            for (NSNumber *device in device_ids)
                if (device.unsignedIntValue != output) mismatched = YES;
            if (!mismatched) continue;
        }
        NSDictionary *identity = wine_identity(pid);
        if (!identity) continue;
        NSMutableDictionary *record = [identity mutableCopy];
        record[@"pid"] = @(pid); record[@"audio_object"] = object; record[@"bundle"] = bundle;
        record[@"running_output"] = @(running);
        NSMutableArray *devices = [NSMutableArray array];
        for (NSNumber *device in device_ids)
            [devices addObject:inventory ? device_info(device.unsignedIntValue) : @{@"id":device}];
        record[@"devices"] = devices;
        [result addObject:record];
    }
    return result;
}

static NSArray<NSNumber *> *routing_targets(NSArray<NSDictionary *> *processes,
                                            AudioDeviceID output) {
    NSMutableArray *targets = [NSMutableArray array];
    for (NSDictionary *process in processes) {
        if (![process[@"running_output"] boolValue]) continue;
        BOOL mismatched = NO;
        for (NSDictionary *device in process[@"devices"])
            if ([device[@"id"] unsignedIntValue] != output) mismatched = YES;
        if (mismatched) [targets addObject:process[@"audio_object"]];
    }
    return targets;
}

typedef struct {
    double sample_rate;
    float gain;
    _Atomic uint64_t frames, bad_buffers;
    UInt32 output_left_channel, output_right_channel, physical_input_streams;
} AudioContext;

// Return a stereo view without allocating, skipping explicitly disabled hardware
// streams. Aggregate input order is the physical subdevice followed by the tap.
static bool stereo_view(const AudioBufferList *list, float **left, float **right,
                        UInt32 *stride, UInt32 *frames, UInt32 skip) {
    if (!list || list->mNumberBuffers <= skip) return false;
    for (UInt32 b = 0; b < skip; ++b) if (list->mBuffers[b].mData) return false;
    UInt32 count = list->mNumberBuffers - skip;
    const AudioBuffer *a = &list->mBuffers[skip];
    if (!a->mData) return false;
    if (count == 1 && a->mNumberChannels == 2) {
        *left = a->mData; *right = *left + 1; *stride = 2; *frames = a->mDataByteSize / 8; return true;
    }
    if (count == 2 && a->mNumberChannels == 1 && a[1].mNumberChannels == 1 && a[1].mData) {
        *left = a->mData; *right = a[1].mData; *stride = 1;
        *frames = MIN(a->mDataByteSize, a[1].mDataByteSize) / 4; return true;
    }
    return false;
}

// Hardware can expose surround channels even when the source tap is stereo.
// Route to the device's preferred stereo pair; leave other channels zeroed.
static bool output_view(AudioBufferList *list, UInt32 left_channel, UInt32 right_channel,
                        float **left, float **right, UInt32 *left_stride,
                        UInt32 *right_stride, UInt32 *frames) {
    *left = NULL; *right = NULL;
    UInt32 first_channel = 1, left_frames = 0, right_frames = 0;
    for (UInt32 b = 0; list && b < list->mNumberBuffers; ++b) {
        AudioBuffer *buffer = &list->mBuffers[b];
        UInt32 channels = buffer->mNumberChannels;
        if (channels && buffer->mData) {
            if (left_channel >= first_channel && left_channel < first_channel + channels) {
                *left = (float *)buffer->mData + left_channel - first_channel;
                *left_stride = channels; left_frames = buffer->mDataByteSize / (4 * channels);
            }
            if (right_channel >= first_channel && right_channel < first_channel + channels) {
                *right = (float *)buffer->mData + right_channel - first_channel;
                *right_stride = channels; right_frames = buffer->mDataByteSize / (4 * channels);
            }
        }
        first_channel += channels;
    }
    *frames = left_frames;
    return *left && *right && left_frames == right_frames;
}

static BOOL float_format(AudioStreamBasicDescription format) {
    UInt32 channels = (format.mFormatFlags & kAudioFormatFlagIsNonInterleaved) ? 1 : format.mChannelsPerFrame;
    return format.mFormatID == kAudioFormatLinearPCM &&
        (format.mFormatFlags & kAudioFormatFlagIsFloat) &&
        (format.mFormatFlags & kAudioFormatFlagIsPacked) &&
        !(format.mFormatFlags & kAudioFormatFlagIsBigEndian) &&
        format.mBitsPerChannel == 32 && channels > 0 && channels <= 128 &&
        format.mBytesPerFrame == channels * sizeof(float) &&
        isfinite(format.mSampleRate) && format.mSampleRate >= 8000;
}

static NSArray *format_signature(AudioDeviceID device) {
    NSMutableArray *signature = [NSMutableArray array];
    for (NSNumber *scope in @[@(kAudioObjectPropertyScopeInput), @(kAudioObjectPropertyScopeOutput)])
        for (NSNumber *stream in object_list(device, kAudioDevicePropertyStreams, scope.unsignedIntValue)) {
            [signature addObject:stream];
            [signature addObject:property(stream.unsignedIntValue, kAudioStreamPropertyVirtualFormat, kAudioObjectPropertyScopeGlobal) ?: NSData.data];
        }
    return signature;
}

static BOOL configure_formats(AudioDeviceID aggregate, AudioDeviceID output, AudioContext *ctx) {
    NSArray *inputs = object_list(aggregate, kAudioDevicePropertyStreams, kAudioObjectPropertyScopeInput);
    NSArray *outputs = object_list(aggregate, kAudioDevicePropertyStreams, kAudioObjectPropertyScopeOutput);
    NSUInteger physical = object_list(output, kAudioDevicePropertyStreams, kAudioObjectPropertyScopeInput).count;
    // A stereo mixdown tap contributes one interleaved stream in this engine.
    if (inputs.count != physical + 1 || !outputs.count || physical > 32) {
        event(@{@"event":@"unsupported_stream_layout", @"physical_inputs":@(physical), @"inputs":inputs, @"outputs":outputs}); return NO;
    }
    ctx->physical_input_streams = (UInt32)physical;
    AudioStreamBasicDescription input_format = {0};
    NSData *data = property([inputs.lastObject unsignedIntValue], kAudioStreamPropertyVirtualFormat, kAudioObjectPropertyScopeGlobal);
    if (data.length != sizeof(input_format)) return NO;
    memcpy(&input_format, data.bytes, sizeof(input_format));
    if (!float_format(input_format) || input_format.mChannelsPerFrame != 2) return NO;
    ctx->sample_rate = input_format.mSampleRate;
    UInt32 total_channels = 0;
    for (NSNumber *stream in outputs) {
        AudioStreamBasicDescription format = {0};
        NSData *value = property(stream.unsignedIntValue, kAudioStreamPropertyVirtualFormat, kAudioObjectPropertyScopeGlobal);
        if (value.length != sizeof(format)) return NO;
        memcpy(&format, value.bytes, sizeof(format));
        if (!float_format(format) || fabs(format.mSampleRate - ctx->sample_rate) > 0.01) {
            event(@{@"event":@"unsupported_output_format", @"output_rate":@(format.mSampleRate), @"input_rate":@(ctx->sample_rate)}); return NO;
        }
        total_channels += format.mChannelsPerFrame;
    }
    return ctx->output_left_channel <= total_channels && ctx->output_right_channel <= total_channels;
}

static BOOL disable_physical_inputs(AudioDeviceID aggregate, AudioDeviceIOProcID io, UInt32 physical) {
    UInt32 count = physical + 1;
    UInt32 size = (UInt32)(offsetof(AudioHardwareIOProcStreamUsage, mStreamIsOn) + count * sizeof(UInt32));
    NSMutableData *data = [NSMutableData dataWithLength:size];
    AudioHardwareIOProcStreamUsage *usage = data.mutableBytes;
    usage->mIOProc = (void *)io; usage->mNumberStreams = count;
    usage->mStreamIsOn[physical] = 1;
    AudioObjectPropertyAddress a = address(kAudioDevicePropertyIOProcStreamUsage, kAudioObjectPropertyScopeInput);
    if (!checked(AudioObjectSetPropertyData(aggregate, &a, 0, NULL, size, usage), "set tap-only input usage")) return NO;
    if (!checked(AudioObjectGetPropertyData(aggregate, &a, 0, NULL, &size, usage), "read tap-only input usage")) return NO;
    if (usage->mNumberStreams != count || !usage->mStreamIsOn[physical]) return NO;
    for (UInt32 i = 0; i < physical; ++i) if (usage->mStreamIsOn[i]) return NO;
    return YES;
}

static OSStatus audio_io(AudioObjectID device, const AudioTimeStamp *now,
                         const AudioBufferList *input, const AudioTimeStamp *input_time,
                         AudioBufferList *output, const AudioTimeStamp *output_time, void *user) {
    (void)device; (void)now; (void)input_time; (void)output_time;
    AudioContext *ctx = user;
    for (UInt32 b = 0; output && b < output->mNumberBuffers; ++b)
        if (output->mBuffers[b].mData) memset(output->mBuffers[b].mData, 0, output->mBuffers[b].mDataByteSize);
    float *left, *right, *out_left, *out_right;
    UInt32 stride, count, ls = 0, rs = 0, out_count;
    // HAL may supply no input while starting. The control thread detects a
    // sustained lack of frames; digital silence is valid progress.
    if (!stereo_view(input, &left, &right, &stride, &count, ctx->physical_input_streams)) return noErr;
    if (!output_view(output, ctx->output_left_channel, ctx->output_right_channel,
                     &out_left, &out_right, &ls, &rs, &out_count) || out_count != count) {
        atomic_fetch_add_explicit(&ctx->bad_buffers, 1, memory_order_relaxed);
        return noErr;
    }
    float step = (float)(1.0 / (0.005 * ctx->sample_rate));
    for (UInt32 i = 0; i < count; ++i) {
        float l = left[i * stride], r = right[i * stride];
        if (!isfinite(l) || !isfinite(r)) { l = 0; r = 0; }
        ctx->gain = fminf(1, ctx->gain + step);
        out_left[i * ls] = fmaxf(-1, fminf(1, l)) * ctx->gain;
        out_right[i * rs] = fmaxf(-1, fminf(1, r)) * ctx->gain;
    }
    atomic_fetch_add_explicit(&ctx->frames, count, memory_order_relaxed);
    return noErr;
}

enum { RouteStopped = 0, RouteFailed = 1, RouteCleanupFailed = 7, RouteChanged = 8, RouteNative = 9 };

static AudioDeviceID default_output(void) {
    return integer(kAudioObjectSystemObject, kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyScopeGlobal);
}

static void state(NSString *value, AudioDeviceID output, NSUInteger processes) {
    event(@{@"event":@"state", @"state":value,
            @"output":string_property(output, kAudioObjectPropertyName) ?: @"",
            @"processes":@(processes)});
}

static int route_once(void) {
    @autoreleasepool {
        uint64_t generation = atomic_load(&output_changes);
        AudioDeviceID output = default_output();
        double rate = sample_rate(output);
        NSArray<NSNumber *> *targets = routing_targets(wine_processes(NO, output), output);
        // Preserve native volume when the game's original output already
        // matches. In particular, avoid downmixing Studio Display's 8 channels
        // onto that same device when headphones disconnect.
        if (!targets.count) { state(@"waiting", output, 0); return RouteNative; }
        if (!output || !isfinite(rate) || rate < 8000) return RouteFailed;
        state(@"connecting", output, targets.count);
        CATapDescription *description = [[CATapDescription alloc] initStereoMixdownOfProcesses:targets];
        description.name = @"CrossPuck Audio";
        description.privateTap = YES;
        description.muteBehavior = CATapMutedWhenTapped;
        AudioObjectID tap = 0, aggregate = 0;
        AudioDeviceIOProcID io = NULL;
        NSArray *signature = nil;
        BOOL started = NO;
        int result = RouteFailed;
        UInt32 stereo[2] = {0};
        NSData *mapping = property(output, kAudioDevicePropertyPreferredChannelsForStereo, kAudioObjectPropertyScopeOutput);
        if (mapping.length != sizeof(stereo)) return RouteFailed;
        memcpy(stereo, mapping.bytes, sizeof(stereo));
        if (!stereo[0] || !stereo[1] || stereo[0] == stereo[1]) return RouteFailed;
        AudioContext *ctx = calloc(1, sizeof(*ctx));
        if (!ctx) return RouteFailed;
        atomic_init(&ctx->frames, 0); atomic_init(&ctx->bad_buffers, 0);
        ctx->output_left_channel = stereo[0]; ctx->output_right_channel = stereo[1];
        if (!atomic_is_lock_free(&ctx->frames)) { free(ctx); return RouteFailed; }
        if (!checked(AudioHardwareCreateProcessTap(description, &tap), "create tap (System Audio Recording permission)")) goto cleanup;
        {
            NSString *uid = string_property(output, kAudioDevicePropertyDeviceUID);
            if (!uid) goto cleanup;
            NSDictionary *spec = @{
                @kAudioAggregateDeviceNameKey:@"CrossPuck Audio",
                @kAudioAggregateDeviceUIDKey:NSUUID.UUID.UUIDString,
                @kAudioAggregateDeviceIsPrivateKey:@YES,
                @kAudioAggregateDeviceIsStackedKey:@NO,
                @kAudioAggregateDeviceMainSubDeviceKey:uid,
                @kAudioAggregateDeviceSubDeviceListKey:@[@{@kAudioSubDeviceUIDKey:uid}],
                @kAudioAggregateDeviceTapListKey:@[@{@kAudioSubTapUIDKey:description.UUID.UUIDString,
                                                    @kAudioSubTapDriftCompensationKey:@YES}],
            };
            if (!checked(AudioHardwareCreateAggregateDevice((__bridge CFDictionaryRef)spec, &aggregate), "create aggregate")) goto cleanup;
        }
        {
            double deadline = monotonic_seconds() + 3;
            while (!stopping && monotonic_seconds() < deadline &&
                    !integer(aggregate, kAudioDevicePropertyDeviceIsAlive, kAudioObjectPropertyScopeGlobal)) pump(0.05);
            if (stopping || !integer(aggregate, kAudioDevicePropertyDeviceIsAlive, kAudioObjectPropertyScopeGlobal)) goto cleanup;
        }
        // HAL drift compensation converts the tap to the aggregate's clock.
        // Reject an incompatible IO layout before starting (and muting) it.
        if (!configure_formats(aggregate, output, ctx)) {
            event(@{@"event":@"error", @"operation":@"unsupported audio format"}); goto cleanup;
        }
        if (!checked(AudioDeviceCreateIOProcID(aggregate, audio_io, ctx, &io), "create IOProc (System Audio Recording permission)")) goto cleanup;
        if (!disable_physical_inputs(aggregate, io, ctx->physical_input_streams)) goto cleanup;
        signature = format_signature(aggregate);
        if (stopping || default_output() != output || atomic_load(&output_changes) != generation) {
            result = RouteChanged; goto cleanup;
        }
        if (!checked(AudioDeviceStart(aggregate, io), "start audio")) goto cleanup;
        started = YES;
        event(@{@"event":@"started", @"targets":targets, @"output_id":@(output), @"sample_rate":@(ctx->sample_rate)});
        {
            double last_progress = monotonic_seconds(), next_scan = 0, next_heartbeat = 0, next_health = 0;
            uint64_t previous_frames = 0;
            BOOL announced = NO;
            while (!stopping) {
                @autoreleasepool {
                    double now = monotonic_seconds();
                    if (now >= next_health) {
                        next_health = now + 0.2;
                        if (default_output() != output || atomic_load(&output_changes) != generation ||
                            !integer(output, kAudioDevicePropertyDeviceIsAlive, kAudioObjectPropertyScopeGlobal) ||
                            fabs(sample_rate(output) - rate) > 0.01) { result = RouteChanged; break; }
                    }
                    if (now >= next_scan) {
                        next_scan = now + 1;
                        if (![signature isEqualToArray:format_signature(aggregate)]) { result = RouteChanged; break; }
                        NSArray *current = routing_targets(wine_processes(NO, output), output);
                        if (![[NSSet setWithArray:current] isEqualToSet:[NSSet setWithArray:targets]]) {
                            result = RouteChanged; break;
                        }
                    }
                    uint64_t frames = atomic_load(&ctx->frames);
                    if (frames != previous_frames) {
                        last_progress = now; previous_frames = frames;
                        if (!announced) { state(@"routing", output, targets.count); announced = YES; }
                    }
                    if (atomic_load(&ctx->bad_buffers) || now - last_progress > 2) {
                        event(@{@"event":@"error", @"operation":@"audio callback stopped or changed format"}); break;
                    }
                    if (now >= next_heartbeat) {
                        event(@{@"event":@"heartbeat", @"frames":@(frames)}); next_heartbeat = now + 1;
                    }
                    pump(fmax(0, next_health - monotonic_seconds()));
                }
            }
            if (stopping) result = RouteStopped;
        }
    cleanup:
        {
            BOOL released = YES, safe = YES;
            if (started) released &= checked(AudioDeviceStop(aggregate, io), "stop audio");
            if (io) safe = checked(AudioDeviceDestroyIOProcID(aggregate, io), "destroy IOProc");
            released &= safe;
            if (aggregate) released &= checked(AudioHardwareDestroyAggregateDevice(aggregate), "destroy aggregate");
            if (tap) released &= checked(AudioHardwareDestroyProcessTap(tap), "destroy tap");
            event(@{@"event":@"cleanup", @"released":@(released), @"frames":@(atomic_load(&ctx->frames)),
                    @"bad_buffers":@(atomic_load(&ctx->bad_buffers)), @"result":@(result)});
            // Never free callback memory while HAL might still reference it.
            // The supervisor restarts the entire helper after cleanup failure.
            if (safe) free(ctx);
            if (!released) return RouteCleanupFailed;
        }
        return result;
    }
}

static void wait_for_change(double seconds, BOOL scan_targets) {
    double until = monotonic_seconds() + seconds, next_scan = 0;
    uint64_t generation = atomic_load(&output_changes);
    AudioDeviceID output = default_output();
    while (!stopping && monotonic_seconds() < until) {
        @autoreleasepool {
            if (atomic_load(&output_changes) != generation) break;
            if (monotonic_seconds() >= next_scan) {
                next_scan = monotonic_seconds() + 1;
                if (default_output() != output) break;
                event(@{@"event":@"heartbeat"});
                if (scan_targets && routing_targets(wine_processes(NO, output), output).count) break;
            }
            pump(fmax(0, fmin(until, next_scan) - monotonic_seconds()));
        }
    }
}

// A per-user lease prevents two copies of CrossPuck from muting/routing the
// same clients. The OS releases this lock on graceful exit or a crash.
static int acquire_lease(void) {
    char directory[PATH_MAX];
    size_t size = confstr(_CS_DARWIN_USER_TEMP_DIR, directory, sizeof(directory));
    if (!size || size > sizeof(directory)) return -1;
    NSString *path = [@(directory) stringByAppendingPathComponent:@"com.github.scryner.crosspuck.audio.lock"];
    int fd = open(path.fileSystemRepresentation, O_CREAT | O_RDWR | O_NOFOLLOW | O_CLOEXEC, 0600);
    if (fd < 0) return -1;
    if (flock(fd, LOCK_EX | LOCK_NB) != 0) { close(fd); return -1; }
    return fd;
}

static int run_routes(void) {
    int lease = acquire_lease();
    if (lease < 0) { state(@"another_instance", 0, 0); return 11; }
    AudioObjectPropertyAddress a = address(kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyScopeGlobal);
    if (!checked(AudioObjectAddPropertyListener(kAudioObjectSystemObject, &a, output_changed, NULL), "watch default output")) {
        close(lease);
        return 1;
    }
    int result = 0, failures = 0;
    while (!stopping) {
        // Retry-state JSON and transition objects must drain each iteration,
        // including failures that occur before route_once starts its own pool.
        @autoreleasepool {
            result = route_once();
            if (result == RouteStopped || result == RouteCleanupFailed || stopping) break;
            if (result == RouteNative) { failures = 0; wait_for_change(30, YES); }
            else if (result == RouteChanged) {
                failures = 0;
                // Require a quiet interval during Bluetooth's transient defaults.
                uint64_t generation;
                do { generation = atomic_load(&output_changes); wait_for_change(0.25, NO); }
                while (!stopping && generation != atomic_load(&output_changes));
            } else {
                state(@"retrying", default_output(), 0);
                double pause = fmin(30, (double)(1u << MIN(failures, 5)));
                failures = MIN(failures + 1, 5);
                wait_for_change(pause, NO);
            }
        }
    }
    checked(AudioObjectRemovePropertyListener(kAudioObjectSystemObject, &a, output_changed, NULL), "remove output watcher");
    close(lease);
    return result == RouteCleanupFailed ? result : 0;
}

static BOOL start_parent_watch(pthread_t *watcher) {
    if (pipe(shutdown_pipe)) return NO;
    for (int i = 0; i < 2; ++i) {
        if (fcntl(shutdown_pipe[i], F_SETFD, FD_CLOEXEC) < 0 ||
            fcntl(shutdown_pipe[i], F_SETFL, O_NONBLOCK) < 0) goto failed;
    }
    if (!pthread_create(watcher, NULL, watch_parent, NULL)) return YES;
failed:
    close(shutdown_pipe[0]); close(shutdown_pipe[1]);
    shutdown_pipe[0] = shutdown_pipe[1] = -1;
    return NO;
}

static void finish_parent_watch(pthread_t watcher) {
    atomic_store(&finished, true);
    notify_shutdown();
    pthread_join(watcher, NULL);
    close(shutdown_pipe[0]); close(shutdown_pipe[1]);
    shutdown_pipe[0] = shutdown_pipe[1] = -1;
}

int crosspuck_audio_main(bool list_only) {
    @autoreleasepool {
        if (list_only) {
            event(@{@"event":@"inventory", @"processes":wine_processes(YES, 0), @"output":device_info(default_output())}); return 0;
        }
        struct stat pipe_stat;
        if (fstat(STDIN_FILENO, &pipe_stat) || !S_ISFIFO(pipe_stat.st_mode)) return 2;
        atomic_init(&stopping, false); atomic_init(&finished, false); atomic_init(&output_changes, 0);
        if (!atomic_is_lock_free(&stopping) || !setup_control_wait()) return 2;
        pthread_t watcher;
        if (!start_parent_watch(&watcher)) { teardown_control_wait(); return 2; }
        signal(SIGINT, interrupt_handler); signal(SIGTERM, interrupt_handler); signal(SIGPIPE, SIG_IGN);
        int result = run_routes();
        // Disable signal writes before closing the self-pipe. Join the watcher
        // before releasing its run-loop source, including all startup errors.
        signal(SIGINT, SIG_IGN); signal(SIGTERM, SIG_IGN);
        finish_parent_watch(watcher);
        teardown_control_wait();
        event(@{@"event":@"stopped"});
        return result;
    }
}
