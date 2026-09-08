// Standalone Core Audio feasibility probe. Production CrossPuck is unchanged.
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

static volatile sig_atomic_t stopping;
static volatile sig_atomic_t gate_requested;
static void interrupt_handler(int sig) { (void)sig; stopping = 1; }
static void gate_handler(int sig) { (void)sig; gate_requested = 1; }
static _Atomic uint64_t output_changes;

static double monotonic_seconds(void) {
    mach_timebase_info_data_t clock;
    mach_timebase_info(&clock);
    return mach_absolute_time() * (double)clock.numer / clock.denom / 1e9;
}

static OSStatus output_changed(AudioObjectID object, UInt32 count,
                                const AudioObjectPropertyAddress *properties, void *context) {
    (void)object; (void)count; (void)properties; (void)context;
    atomic_fetch_add(&output_changes, 1);
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
    NSData *data = property(object, selector, scope);
    UInt32 value = 0;
    if (data.length == sizeof(value)) memcpy(&value, data.bytes, sizeof(value));
    return value;
}

static double sample_rate(AudioObjectID object) {
    NSData *data = property(object, kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal);
    double rate = 0;
    if (data.length == sizeof(rate)) memcpy(&rate, data.bytes, sizeof(rate));
    return rate;
}

static NSString *string_property(AudioObjectID object, AudioObjectPropertySelector selector) {
    NSData *data = property(object, selector, kAudioObjectPropertyScopeGlobal);
    CFStringRef value = NULL;
    if (data.length != sizeof(value)) return nil;
    memcpy(&value, data.bytes, sizeof(value));
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

// Read only argv and WINEPREFIX for an already identified Core Audio Wine client.
// Other environment variables are neither retained nor logged.
static NSDictionary *wine_identity(pid_t pid) {
    int mib[] = {CTL_KERN, KERN_PROCARGS2, pid};
    size_t size = 0;
    if (sysctl(mib, 3, NULL, &size, NULL, 0) != 0 || size > 1024 * 1024) return nil;
    NSMutableData *data = [NSMutableData dataWithLength:size];
    if (sysctl(mib, 3, data.mutableBytes, &size, NULL, 0) != 0 || size < sizeof(int)) return nil;
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
    prefix = prefix.stringByResolvingSymlinksInPath.stringByStandardizingPath;
    if (![[NSFileManager defaultManager] fileExistsAtPath:[prefix stringByAppendingPathComponent:@"cxbottle.conf"]]) return nil;
    return @{@"command":command ?: @"", @"bottle_path":prefix};
}

static NSDictionary *device_info(AudioDeviceID device) {
    return @{@"id":@(device), @"name":string_property(device, kAudioObjectPropertyName) ?: @"",
             @"uid":string_property(device, kAudioDevicePropertyDeviceUID) ?: @"",
             @"sample_rate":@(sample_rate(device)),
             @"input_streams":object_list(device, kAudioDevicePropertyStreams, kAudioObjectPropertyScopeInput),
             @"output_streams":object_list(device, kAudioDevicePropertyStreams, kAudioObjectPropertyScopeOutput)};
}

static NSArray<NSDictionary *> *wine_processes(void) {
    NSMutableArray *result = [NSMutableArray array];
    for (NSNumber *object in object_list(kAudioObjectSystemObject, kAudioHardwarePropertyProcessObjectList, kAudioObjectPropertyScopeGlobal)) {
        AudioObjectID oid = object.unsignedIntValue;
        NSString *bundle = string_property(oid, kAudioProcessPropertyBundleID);
        if (![bundle hasPrefix:@"com.codeweavers.CrossOver."]) continue;
        pid_t pid = (pid_t)integer(oid, kAudioProcessPropertyPID, kAudioObjectPropertyScopeGlobal);
        if (pid == getpid()) continue;
        NSDictionary *identity = wine_identity(pid);
        if (!identity) continue;
        NSMutableDictionary *record = [identity mutableCopy];
        record[@"pid"] = @(pid); record[@"audio_object"] = object; record[@"bundle"] = bundle;
        record[@"running_output"] = @(integer(oid, kAudioProcessPropertyIsRunningOutput, kAudioObjectPropertyScopeGlobal) != 0);
        NSMutableArray *devices = [NSMutableArray array];
        for (NSNumber *device in object_list(oid, kAudioProcessPropertyDevices, kAudioObjectPropertyScopeOutput))
            [devices addObject:device_info(device.unsignedIntValue)];
        record[@"devices"] = devices;
        [result addObject:record];
    }
    return result;
}

static NSArray<NSNumber *> *routing_targets(NSArray<NSDictionary *> *processes,
                                            AudioDeviceID output, pid_t selected, BOOL mismatched_only) {
    NSMutableArray *targets = [NSMutableArray array];
    for (NSDictionary *process in processes) {
        if (![process[@"running_output"] boolValue] || (selected && [process[@"pid"] intValue] != selected)) continue;
        BOOL mismatched = NO;
        for (NSDictionary *device in process[@"devices"])
            if ([device[@"id"] unsignedIntValue] != output) mismatched = YES;
        if (!mismatched_only || mismatched) [targets addObject:process[@"audio_object"]];
    }
    return targets;
}

typedef struct {
    BOOL route;
    double sample_rate;
    float *capture;
    uint64_t capture_capacity, stored_frames;
    double local_energy;
    float local_peak, gain;
    _Atomic bool gate;
    _Atomic uint64_t callbacks, frames, audible_frames, forwarded_frames, gated_frames, bad_buffers;
    _Atomic uint64_t max_callback_ticks, timestamp_gap_ticks;
    _Atomic double energy;
    _Atomic float peak;
    UInt32 input_buffers, output_buffers;
    UInt32 output_left_channel, output_right_channel;
    UInt32 physical_input_streams;
    UInt32 input_channels[8], input_bytes[8], output_channels[8], output_bytes[8];
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

static OSStatus audio_io(AudioObjectID device, const AudioTimeStamp *now,
                         const AudioBufferList *input, const AudioTimeStamp *input_time,
                         AudioBufferList *output, const AudioTimeStamp *output_time, void *user) {
    (void)device; (void)now;
    AudioContext *ctx = user;
    uint64_t start = mach_absolute_time();
    if (atomic_fetch_add_explicit(&ctx->callbacks, 1, memory_order_relaxed) == 0) {
        ctx->input_buffers = input ? input->mNumberBuffers : 0;
        ctx->output_buffers = output ? output->mNumberBuffers : 0;
        for (UInt32 b = 0; b < MIN(ctx->input_buffers, 8); ++b) {
            ctx->input_channels[b] = input->mBuffers[b].mNumberChannels;
            ctx->input_bytes[b] = input->mBuffers[b].mDataByteSize;
        }
        for (UInt32 b = 0; b < MIN(ctx->output_buffers, 8); ++b) {
            ctx->output_channels[b] = output->mBuffers[b].mNumberChannels;
            ctx->output_bytes[b] = output->mBuffers[b].mDataByteSize;
        }
    }
    for (UInt32 b = 0; output && b < output->mNumberBuffers; ++b)
        if (output->mBuffers[b].mData) memset(output->mBuffers[b].mData, 0, output->mBuffers[b].mDataByteSize);
    float *left, *right, *out_left = NULL, *out_right = NULL;
    UInt32 stride, count, out_left_stride = 0, out_right_stride = 0, out_count = 0;
    if (!stereo_view(input, &left, &right, &stride, &count, ctx->physical_input_streams)) return noErr;
    if (ctx->route && (!output_view(output, ctx->output_left_channel, ctx->output_right_channel,
                                    &out_left, &out_right, &out_left_stride, &out_right_stride, &out_count) || out_count != count)) {
        atomic_fetch_add_explicit(&ctx->bad_buffers, 1, memory_order_relaxed); return noErr;
    }
    bool gate = atomic_load_explicit(&ctx->gate, memory_order_relaxed);
    uint64_t audible = 0;
    float gain_step = (float)(1.0 / (0.005 * ctx->sample_rate));
    for (UInt32 i = 0; i < count; ++i) {
        float l = left[i * stride], r = right[i * stride];
        if (!isfinite(l) || !isfinite(r)) { l = 0; r = 0; }
        float peak = fmaxf(fabsf(l), fabsf(r));
        if (peak > 0.000001f) ++audible;
        if (peak > ctx->local_peak) ctx->local_peak = peak;
        ctx->local_energy += (double)l * l + (double)r * r;
        if (ctx->capture && ctx->stored_frames < ctx->capture_capacity) {
            ctx->capture[ctx->stored_frames * 2] = l;
            ctx->capture[ctx->stored_frames * 2 + 1] = r; ++ctx->stored_frames;
        }
        if (ctx->route) {
            ctx->gain = gate ? fmaxf(0, ctx->gain - gain_step) : fminf(1, ctx->gain + gain_step);
            out_left[i * out_left_stride] = fmaxf(-1, fminf(1, l)) * ctx->gain;
            out_right[i * out_right_stride] = fmaxf(-1, fminf(1, r)) * ctx->gain;
        }
    }
    atomic_fetch_add_explicit(&ctx->frames, count, memory_order_relaxed);
    atomic_fetch_add_explicit(&ctx->audible_frames, audible, memory_order_relaxed);
    atomic_store_explicit(&ctx->energy, ctx->local_energy, memory_order_relaxed);
    atomic_store_explicit(&ctx->peak, ctx->local_peak, memory_order_relaxed);
    if (ctx->route) atomic_fetch_add_explicit(gate ? &ctx->gated_frames : &ctx->forwarded_frames, count, memory_order_relaxed);
    if ((input_time->mFlags & kAudioTimeStampHostTimeValid) && (output_time->mFlags & kAudioTimeStampHostTimeValid) && output_time->mHostTime >= input_time->mHostTime)
        atomic_store_explicit(&ctx->timestamp_gap_ticks, output_time->mHostTime - input_time->mHostTime, memory_order_relaxed);
    uint64_t elapsed = mach_absolute_time() - start;
    if (elapsed > atomic_load_explicit(&ctx->max_callback_ticks, memory_order_relaxed))
        atomic_store_explicit(&ctx->max_callback_ticks, elapsed, memory_order_relaxed);
    return noErr;
}

static NSDictionary *metrics(AudioContext *ctx, mach_timebase_info_data_t clock) {
    uint64_t frames = atomic_load(&ctx->frames);
    double rms = frames ? sqrt(atomic_load(&ctx->energy) / (frames * 2)) : 0;
    double tick_ms = (double)clock.numer / clock.denom / 1e6;
    return @{@"callbacks":@(atomic_load(&ctx->callbacks)), @"frames":@(frames),
             @"audible_frames":@(atomic_load(&ctx->audible_frames)), @"peak":@(atomic_load(&ctx->peak)),
             @"rms":@(rms), @"forwarded_frames":@(atomic_load(&ctx->forwarded_frames)),
             @"gated_frames":@(atomic_load(&ctx->gated_frames)), @"bad_buffers":@(atomic_load(&ctx->bad_buffers)),
             @"max_callback_ms":@(atomic_load(&ctx->max_callback_ticks) * tick_ms),
             @"hal_timestamp_gap_ms":@(atomic_load(&ctx->timestamp_gap_ticks) * tick_ms)};
}

static BOOL write_wav(NSString *path, AudioContext *ctx) {
    FILE *file = fopen(path.fileSystemRepresentation, "wb");
    if (!file) return NO;
    uint32_t size = (uint32_t)(ctx->stored_frames * 2 * sizeof(float)), riff_size = size + 36;
    uint32_t fmt_size = 16, rate = (uint32_t)llround(ctx->sample_rate), byte_rate = rate * 8;
    uint16_t format = 3, channels = 2, align = 8, bits = 32;
    fwrite("RIFF", 1, 4, file); fwrite(&riff_size, 4, 1, file); fwrite("WAVEfmt ", 1, 8, file);
    fwrite(&fmt_size, 4, 1, file); fwrite(&format, 2, 1, file); fwrite(&channels, 2, 1, file);
    fwrite(&rate, 4, 1, file); fwrite(&byte_rate, 4, 1, file); fwrite(&align, 2, 1, file); fwrite(&bits, 2, 1, file);
    fwrite("data", 1, 4, file); fwrite(&size, 4, 1, file); fwrite(ctx->capture, 1, size, file);
    BOOL ok = !ferror(file); return fclose(file) == 0 && ok;
}

static BOOL configure_formats(AudioDeviceID aggregate, AudioDeviceID output, AudioContext *ctx) {
    NSArray *inputs = object_list(aggregate, kAudioDevicePropertyStreams, kAudioObjectPropertyScopeInput);
    NSArray *outputs = object_list(aggregate, kAudioDevicePropertyStreams, kAudioObjectPropertyScopeOutput);
    NSUInteger physical = object_list(output, kAudioDevicePropertyStreams, kAudioObjectPropertyScopeInput).count;
    // A stereo mixdown tap contributes one interleaved stream in this PoC.
    if (inputs.count != physical + 1 || !outputs.count || physical > 32) {
        event(@{@"event":@"unsupported_stream_layout", @"physical_inputs":@(physical), @"inputs":inputs, @"outputs":outputs}); return NO;
    }
    ctx->physical_input_streams = (UInt32)physical;
    AudioStreamBasicDescription input_format = {0};
    NSData *data = property([inputs.lastObject unsignedIntValue], kAudioStreamPropertyVirtualFormat, kAudioObjectPropertyScopeGlobal);
    if (data.length != sizeof(input_format)) return NO;
    memcpy(&input_format, data.bytes, sizeof(input_format));
    if (input_format.mFormatID != kAudioFormatLinearPCM || !(input_format.mFormatFlags & kAudioFormatFlagIsFloat) || input_format.mBitsPerChannel != 32 || input_format.mChannelsPerFrame != 2 || !isfinite(input_format.mSampleRate) || input_format.mSampleRate < 8000) return NO;
    ctx->sample_rate = input_format.mSampleRate;
    for (NSNumber *stream in outputs) {
        AudioStreamBasicDescription format = {0};
        NSData *value = property(stream.unsignedIntValue, kAudioStreamPropertyVirtualFormat, kAudioObjectPropertyScopeGlobal);
        if (value.length != sizeof(format)) return NO;
        memcpy(&format, value.bytes, sizeof(format));
        if (format.mFormatID != kAudioFormatLinearPCM || !(format.mFormatFlags & kAudioFormatFlagIsFloat) || format.mBitsPerChannel != 32 || fabs(format.mSampleRate - ctx->sample_rate) > 0.01) {
            event(@{@"event":@"unsupported_output_format", @"output_rate":@(format.mSampleRate), @"input_rate":@(ctx->sample_rate)}); return NO;
        }
    }
    return YES;
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
    event(@{@"event":@"input_stream_usage", @"disabled_physical_streams":@(physical), @"enabled_tap_streams":@1});
    return YES;
}

static int run_probe(NSString *mode, NSString *wav, double seconds,
                     double gate_after, double gate_for, AudioDeviceID output_id,
                     pid_t selected_pid, BOOL follow, double deadline) {
    @autoreleasepool {
        mach_timebase_info_data_t clock; mach_timebase_info(&clock);
        uint64_t initial_changes = atomic_load(&output_changes);
        if (!output_id) output_id = integer(kAudioObjectSystemObject, kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyScopeGlobal);
        double output_rate = sample_rate(output_id);
        NSArray *processes = wine_processes();
        event(@{@"event":@"inventory", @"processes":processes, @"output":device_info(output_id), @"poc_pid":@(getpid())});
        if ([mode isEqual:@"list"]) return output_id ? 0 : 1;
        NSArray<NSNumber *> *targets = routing_targets(processes, output_id, selected_pid, follow);
        if (!targets.count) {
            event(@{@"event":follow ? @"passthrough" : @"no_active_crossover_audio", @"output_id":@(output_id)});
            return follow ? 9 : 3;
        }
        if (!output_id || !isfinite(output_rate) || output_rate < 8000) {
            event(@{@"event":@"unsupported_output", @"reason":@"No ready output device"}); return 4;
        }
        BOOL route = [mode isEqual:@"route"];
        CATapDescription *description = [[CATapDescription alloc] initStereoMixdownOfProcesses:targets];
        description.name = @"CrossPuck Audio PoC"; description.privateTap = YES;
        description.muteBehavior = route ? CATapMutedWhenTapped : CATapUnmuted;
        AudioObjectID tap = kAudioObjectUnknown, aggregate = kAudioObjectUnknown;
        AudioDeviceIOProcID io = NULL;
        BOOL started = NO, restart = NO, failed = NO; int result = 1;
        NSData *format_data = nil;
        NSData *stereo_data = property(output_id, kAudioDevicePropertyPreferredChannelsForStereo, kAudioObjectPropertyScopeOutput);
        UInt32 stereo_channels[2] = {0};
        if (stereo_data.length != sizeof(stereo_channels)) {
            event(@{@"event":@"unsupported_output", @"reason":@"Preferred stereo channel mapping is unavailable"}); return 4;
        }
        memcpy(stereo_channels, stereo_data.bytes, sizeof(stereo_channels));
        if (!stereo_channels[0] || !stereo_channels[1] || stereo_channels[0] == stereo_channels[1]) return 4;
        AudioContext *ctx = calloc(1, sizeof(*ctx));
        if (!ctx) return 4;
        atomic_init(&ctx->gate, false);
        atomic_init(&ctx->callbacks, 0); atomic_init(&ctx->frames, 0);
        atomic_init(&ctx->audible_frames, 0); atomic_init(&ctx->forwarded_frames, 0);
        atomic_init(&ctx->gated_frames, 0); atomic_init(&ctx->bad_buffers, 0);
        atomic_init(&ctx->max_callback_ticks, 0); atomic_init(&ctx->timestamp_gap_ticks, 0);
        atomic_init(&ctx->energy, 0); atomic_init(&ctx->peak, 0);
        ctx->route = route;
        ctx->output_left_channel = stereo_channels[0]; ctx->output_right_channel = stereo_channels[1];
        if (!atomic_is_lock_free(&ctx->frames) || !atomic_is_lock_free(&ctx->energy)) { free(ctx); return 4; }
        if (!checked(AudioHardwareCreateProcessTap(description, &tap), "create tap")) goto cleanup;
        AudioStreamBasicDescription tap_format = {0};
        format_data = property(tap, kAudioTapPropertyFormat, kAudioObjectPropertyScopeGlobal);
        if (format_data.length != sizeof(tap_format)) goto cleanup;
        memcpy(&tap_format, format_data.bytes, sizeof(tap_format));
        if (tap_format.mFormatID != kAudioFormatLinearPCM || !(tap_format.mFormatFlags & kAudioFormatFlagIsFloat) || tap_format.mBitsPerChannel != 32 || tap_format.mChannelsPerFrame != 2) {
            event(@{@"event":@"unsupported_tap_format"}); goto cleanup;
        }
        ctx->sample_rate = tap_format.mSampleRate;
        {
            NSString *uid = string_property(output_id, kAudioDevicePropertyDeviceUID);
            if (!uid) goto cleanup;
            NSDictionary *aggregate_description = @{
                @kAudioAggregateDeviceNameKey:@"CrossPuck Audio PoC",
                @kAudioAggregateDeviceUIDKey:NSUUID.UUID.UUIDString,
                @kAudioAggregateDeviceIsPrivateKey:@YES,
                @kAudioAggregateDeviceIsStackedKey:@NO,
                @kAudioAggregateDeviceMainSubDeviceKey:uid,
                @kAudioAggregateDeviceSubDeviceListKey:@[@{@kAudioSubDeviceUIDKey:uid}],
                @kAudioAggregateDeviceTapListKey:@[@{@kAudioSubTapUIDKey:description.UUID.UUIDString,
                                                    @kAudioSubTapDriftCompensationKey:@YES}],
            };
            if (!checked(AudioHardwareCreateAggregateDevice((__bridge CFDictionaryRef)aggregate_description, &aggregate), "create aggregate")) goto cleanup;
        }
        {
            double ready_deadline = monotonic_seconds() + 3;
            while (!integer(aggregate, kAudioDevicePropertyDeviceIsAlive, kAudioObjectPropertyScopeGlobal) && monotonic_seconds() < ready_deadline && !stopping)
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.05, false);
            if (!integer(aggregate, kAudioDevicePropertyDeviceIsAlive, kAudioObjectPropertyScopeGlobal)) goto cleanup;
        }
        // Use the aggregate IO format: HAL may resample the tap to its main clock.
        if (!configure_formats(aggregate, output_id, ctx)) goto cleanup;
        if (wav) {
            ctx->capture_capacity = (uint64_t)ceil((seconds + 1) * ctx->sample_rate);
            ctx->capture = calloc((size_t)ctx->capture_capacity * 2, sizeof(float));
            if (!ctx->capture) goto cleanup;
        }
        if (!checked(AudioDeviceCreateIOProcID(aggregate, audio_io, ctx, &io), "create IOProc")) goto cleanup;
        if (!disable_physical_inputs(aggregate, io, ctx->physical_input_streams)) goto cleanup;
        event(@{@"event":@"prepared", @"mode":mode, @"targets":targets, @"tap":@(tap), @"aggregate":@(aggregate), @"sample_rate":@(ctx->sample_rate),
                @"output_id":@(output_id), @"output_rate":@(output_rate), @"source_tap_rate":@(tap_format.mSampleRate),
                @"stereo_channels":@[@(stereo_channels[0]), @(stereo_channels[1])],
                @"buffer_frames":@(integer(aggregate, kAudioDevicePropertyBufferFrameSize, kAudioObjectPropertyScopeGlobal)), @"seconds":@(seconds)});
        if (stopping || (follow && monotonic_seconds() >= deadline)) goto cleanup;
        if (!checked(AudioDeviceStart(aggregate, io), "start audio")) goto cleanup;
        started = YES;
        event(@{@"event":@"started", @"mode":mode});
        {
            uint64_t start = mach_absolute_time();
            double next_log = 0, next_health = 0, next_targets = 1, last_progress = 0, requested_gate_until = -1;
            uint64_t previous_frames = 0;
            BOOL previous_gate = NO;
            while (!stopping) {
                double elapsed = (mach_absolute_time() - start) * (double)clock.numer / clock.denom / 1e9;
                if (elapsed >= seconds || (follow && monotonic_seconds() >= deadline)) break;
                if (follow && elapsed >= next_health) {
                    next_health = elapsed + 0.2;
                    AudioDeviceID current = integer(kAudioObjectSystemObject, kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyScopeGlobal);
                    if (current != output_id || atomic_load(&output_changes) != initial_changes ||
                        !integer(output_id, kAudioDevicePropertyDeviceIsAlive, kAudioObjectPropertyScopeGlobal) ||
                        fabs(sample_rate(output_id) - output_rate) > 0.01) {
                        event(@{@"event":@"output_change_detected", @"from":@(output_id), @"to":@(current), @"listener_notifications":@(atomic_load(&output_changes)), @"elapsed":@(elapsed)});
                        restart = YES; break;
                    }
                }
                if (follow && elapsed >= next_targets) {
                    next_targets = elapsed + 1;
                    NSArray *current_targets = routing_targets(wine_processes(), output_id, selected_pid, YES);
                    if (![[NSSet setWithArray:current_targets] isEqualToSet:[NSSet setWithArray:targets]]) {
                        event(@{@"event":@"targets_changed", @"from":targets, @"to":current_targets});
                        restart = YES; break;
                    }
                }
                if (gate_requested) { gate_requested = 0; requested_gate_until = elapsed + gate_for; }
                BOOL gate = route && ((gate_after >= 0 && elapsed >= gate_after && elapsed < gate_after + gate_for) || elapsed < requested_gate_until);
                atomic_store(&ctx->gate, gate);
                if (gate != previous_gate) {
                    event(@{@"event":gate ? @"gate_silent" : @"gate_forwarding", @"elapsed":@(elapsed)}); previous_gate = gate;
                }
                if (elapsed >= next_log) {
                    event(@{@"event":@"meter", @"elapsed":@(elapsed), @"metrics":metrics(ctx, clock)}); next_log += 1;
                }
                uint64_t frames = atomic_load(&ctx->frames);
                if (frames != previous_frames) { previous_frames = frames; last_progress = elapsed; }
                if (atomic_load(&ctx->bad_buffers) || (route && elapsed - last_progress > 2)) {
                    event(@{@"event":@"playback_health_failure", @"elapsed":@(elapsed)});
                    failed = YES; break;
                }
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.05, false);
                usleep(10000);
            }
        }
        result = restart ? 8 : (!failed && atomic_load(&ctx->audible_frames) > 0 && !atomic_load(&ctx->bad_buffers) ? 0 : 5);
    cleanup:
        {
            BOOL safe_to_free = YES, released = YES;
            if (started) released &= checked(AudioDeviceStop(aggregate, io), "stop audio");
            if (io) safe_to_free = checked(AudioDeviceDestroyIOProcID(aggregate, io), "destroy IOProc");
            released &= safe_to_free;
            if (aggregate) released &= checked(AudioHardwareDestroyAggregateDevice(aggregate), "destroy aggregate");
            if (tap) released &= checked(AudioHardwareDestroyProcessTap(tap), "destroy tap");
            if (!released) result = 7;
            event(@{@"event":@"cleanup", @"released":@(released), @"metrics":metrics(ctx, clock), @"result":@(result)});
            // If HAL could not stop its callback, retain storage until process
            // exit rather than racing it with destruction or WAV access.
            if (!safe_to_free) return result;
            NSMutableArray *inputs = [NSMutableArray array], *outputs = [NSMutableArray array];
            for (UInt32 b = 0; b < MIN(ctx->input_buffers, 8); ++b)
                [inputs addObject:@{@"channels":@(ctx->input_channels[b]), @"bytes":@(ctx->input_bytes[b])}];
            for (UInt32 b = 0; b < MIN(ctx->output_buffers, 8); ++b)
                [outputs addObject:@{@"channels":@(ctx->output_channels[b]), @"bytes":@(ctx->output_bytes[b])}];
            event(@{@"event":@"buffer_layout", @"inputs":inputs, @"outputs":outputs});
        }
        if (wav && ctx->capture && ctx->stored_frames) {
            BOOL ok = write_wav(wav, ctx);
            event(@{@"event":@"wav", @"path":wav, @"frames":@(ctx->stored_frames), @"ok":@(ok)});
            if (!ok) result = 6;
        }
        free(ctx->capture); free(ctx);
        return result;
    }
}

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        NSString *mode = @"list", *wav = nil, *log = nil;
        double seconds = 15, gate_after = -1, gate_for = 10;
        AudioDeviceID output_id = 0; pid_t selected_pid = 0;
        BOOL follow = NO;
        for (int i = 1; i < argc; ++i) {
            NSString *arg = @(argv[i]);
            if ([arg isEqual:@"--list"]) mode = @"list";
            else if ([arg isEqual:@"--capture"]) mode = @"capture";
            else if ([arg isEqual:@"--route"]) mode = @"route";
            else if ([arg isEqual:@"--follow-default"]) { follow = YES; mode = @"route"; }
            else if (i + 1 < argc && [arg isEqual:@"--seconds"]) seconds = atof(argv[++i]);
            else if (i + 1 < argc && [arg isEqual:@"--gate-after"]) gate_after = atof(argv[++i]);
            else if (i + 1 < argc && [arg isEqual:@"--gate-for"]) gate_for = atof(argv[++i]);
            else if (i + 1 < argc && [arg isEqual:@"--pid"]) selected_pid = atoi(argv[++i]);
            else if (i + 1 < argc && [arg isEqual:@"--output-id"]) output_id = (AudioDeviceID)strtoul(argv[++i], NULL, 10);
            else if (i + 1 < argc && [arg isEqual:@"--wav"]) wav = @(argv[++i]);
            else if (i + 1 < argc && [arg isEqual:@"--log"]) log = @(argv[++i]);
            else { fprintf(stderr, "Unknown/incomplete argument: %s\n", argv[i]); return 2; }
        }
        if (!isfinite(seconds) || !isfinite(gate_for) || !isfinite(gate_after) || seconds < 1 || seconds > (follow ? 900 : 300) || gate_for < 0 || gate_for > 300 || (follow && (output_id || wav || ![mode isEqual:@"route"]))) return 2;
        if (log && !freopen(log.fileSystemRepresentation, "w", stdout)) return 2;
        signal(SIGINT, interrupt_handler); signal(SIGTERM, interrupt_handler); signal(SIGUSR1, gate_handler);
        atomic_init(&output_changes, 0);
        if (!follow) return run_probe(mode, wav, seconds, gate_after, gate_for, output_id, selected_pid, NO, 0);
        AudioObjectPropertyAddress a = address(kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyScopeGlobal);
        if (!checked(AudioObjectAddPropertyListener(kAudioObjectSystemObject, &a, output_changed, NULL), "watch default output")) return 1;
        double deadline = monotonic_seconds() + seconds;
        int result = 1, failures = 0;
        UInt32 sessions = 0, rebuilds = 0, passthroughs = 0;
        event(@{@"event":@"follow_started", @"poc_pid":@(getpid()), @"seconds":@(seconds)});
        while (!stopping && monotonic_seconds() < deadline) {
            ++sessions;
            result = run_probe(mode, nil, deadline - monotonic_seconds(), gate_after, gate_for, 0, selected_pid, YES, deadline);
            if (result == 0 || result == 7 || stopping) break;
            if (result == 9) {
                ++passthroughs; failures = 0;
                uint64_t generation = atomic_load(&output_changes);
                double next_scan = monotonic_seconds();
                while (!stopping && monotonic_seconds() < deadline) {
                    CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.05, false);
                    usleep(10000);
                    if (atomic_load(&output_changes) != generation) break;
                    if (monotonic_seconds() >= next_scan) {
                        next_scan = monotonic_seconds() + 1;
                        AudioDeviceID current = integer(kAudioObjectSystemObject, kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyScopeGlobal);
                        if (routing_targets(wine_processes(), current, selected_pid, YES).count) break;
                    }
                }
                continue;
            }
            if (result == 8) { ++rebuilds; failures = 0; }
            else if (++failures >= 3) break;
            event(@{@"event":@"route_rebuild", @"previous_result":@(result), @"rebuilds":@(rebuilds), @"attempt_failures":@(failures)});
            // Debounce device changes; failed attempts get bounded backoff while
            // the original game path is unmuted by complete teardown above.
            double pause = result == 8 ? 0.25 : (double)failures;
            double stable_since = monotonic_seconds();
            uint64_t generation = atomic_load(&output_changes);
            while (!stopping && monotonic_seconds() < deadline && monotonic_seconds() - stable_since < pause) {
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.05, false);
                usleep(10000);
                uint64_t current = atomic_load(&output_changes);
                if (current != generation) { stable_since = monotonic_seconds(); generation = current; }
            }
        }
        if (result == 9) result = 0;
        if (!checked(AudioObjectRemovePropertyListener(kAudioObjectSystemObject, &a, output_changed, NULL), "remove output watcher")) result = 7;
        event(@{@"event":@"follow_finished", @"sessions":@(sessions), @"rebuilds":@(rebuilds), @"passthrough_intervals":@(passthroughs), @"result":@(result)});
        return result;
    }
}
