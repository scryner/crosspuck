#include <CoreFoundation/CoreFoundation.h>
#include <IOKit/hid/IOHIDElement.h>
#include <IOKit/hid/IOHIDKeys.h>
#include <IOKit/hid/IOHIDLib.h>
#include <IOKit/hid/IOHIDValue.h>
#include <dlfcn.h>
#include <pthread.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#include <unistd.h>

#define INTERPOSE(replacement, replacee)                                           \
    __attribute__((used)) static struct {                                          \
        const void *replacement;                                                   \
        const void *replacee;                                                      \
    } interpose_##replacee __attribute__((section("__DATA,__interpose"))) = {      \
        (const void *)(unsigned long)&replacement,                                  \
        (const void *)(unsigned long)&replacee                                      \
    }

typedef IOReturn (*IOHIDDeviceOpenFn)(IOHIDDeviceRef device, IOOptionBits options);
typedef IOReturn (*IOHIDDeviceGetReportFn)(
    IOHIDDeviceRef device,
    IOHIDReportType report_type,
    CFIndex report_id,
    uint8_t *report,
    CFIndex *report_length);
typedef IOReturn (*IOHIDDeviceSetReportFn)(
    IOHIDDeviceRef device,
    IOHIDReportType report_type,
    CFIndex report_id,
    const uint8_t *report,
    CFIndex report_length);
typedef IOReturn (*IOHIDDeviceSetReportWithCallbackFn)(
    IOHIDDeviceRef device,
    IOHIDReportType report_type,
    CFIndex report_id,
    const uint8_t *report,
    CFIndex report_length,
    CFTimeInterval timeout,
    IOHIDReportCallback callback,
    void *context);
typedef IOReturn (*IOHIDDeviceGetReportWithCallbackFn)(
    IOHIDDeviceRef device,
    IOHIDReportType report_type,
    CFIndex report_id,
    uint8_t *report,
    CFIndex *report_length,
    CFTimeInterval timeout,
    IOHIDReportCallback callback,
    void *context);
typedef IOReturn (*IOHIDDeviceSetValueFn)(
    IOHIDDeviceRef device,
    IOHIDElementRef element,
    IOHIDValueRef value);
typedef IOReturn (*IOHIDDeviceSetValueMultipleFn)(
    IOHIDDeviceRef device,
    CFDictionaryRef multiple);
typedef IOReturn (*IOHIDDeviceSetValueWithCallbackFn)(
    IOHIDDeviceRef device,
    IOHIDElementRef element,
    IOHIDValueRef value,
    CFTimeInterval timeout,
    IOHIDValueCallback callback,
    void *context);
typedef IOReturn (*IOHIDDeviceSetValueMultipleWithCallbackFn)(
    IOHIDDeviceRef device,
    CFDictionaryRef multiple,
    CFTimeInterval timeout,
    IOHIDValueMultipleCallback callback,
    void *context);
typedef void (*IOHIDDeviceRegisterInputReportCallbackFn)(
    IOHIDDeviceRef device,
    uint8_t *report,
    CFIndex report_length,
    IOHIDReportCallback callback,
    void *context);

typedef struct InputReportCallbackContext {
    IOHIDDeviceRef device;
    IOHIDReportCallback callback;
    void *context;
} InputReportCallbackContext;

static pthread_mutex_t g_log_mutex = PTHREAD_MUTEX_INITIALIZER;
static FILE *g_log_file = NULL;
static int g_filter_vid = 0x28DE;
static int g_filter_pid = 0x1304;
static bool g_log_all = false;
static bool g_jsonl = false;
static bool g_log_get = true;
static bool g_log_input = true;
static bool g_log_value = true;
static size_t g_max_bytes = 128;

static IOHIDDeviceOpenFn real_IOHIDDeviceOpen = NULL;
static IOHIDDeviceGetReportFn real_IOHIDDeviceGetReport = NULL;
static IOHIDDeviceSetReportFn real_IOHIDDeviceSetReport = NULL;
static IOHIDDeviceSetReportWithCallbackFn
    real_IOHIDDeviceSetReportWithCallback = NULL;
static IOHIDDeviceGetReportWithCallbackFn
    real_IOHIDDeviceGetReportWithCallback = NULL;
static IOHIDDeviceSetValueFn real_IOHIDDeviceSetValue = NULL;
static IOHIDDeviceSetValueMultipleFn real_IOHIDDeviceSetValueMultiple = NULL;
static IOHIDDeviceSetValueWithCallbackFn
    real_IOHIDDeviceSetValueWithCallback = NULL;
static IOHIDDeviceSetValueMultipleWithCallbackFn
    real_IOHIDDeviceSetValueMultipleWithCallback = NULL;
static IOHIDDeviceRegisterInputReportCallbackFn
    real_IOHIDDeviceRegisterInputReportCallback = NULL;

IOReturn crosspuck_IOHIDDeviceOpen(IOHIDDeviceRef device, IOOptionBits options);
IOReturn crosspuck_IOHIDDeviceGetReport(
    IOHIDDeviceRef device,
    IOHIDReportType report_type,
    CFIndex report_id,
    uint8_t *report,
    CFIndex *report_length);
IOReturn crosspuck_IOHIDDeviceSetReport(
    IOHIDDeviceRef device,
    IOHIDReportType report_type,
    CFIndex report_id,
    const uint8_t *report,
    CFIndex report_length);
IOReturn crosspuck_IOHIDDeviceSetReportWithCallback(
    IOHIDDeviceRef device,
    IOHIDReportType report_type,
    CFIndex report_id,
    const uint8_t *report,
    CFIndex report_length,
    CFTimeInterval timeout,
    IOHIDReportCallback callback,
    void *context);
IOReturn crosspuck_IOHIDDeviceGetReportWithCallback(
    IOHIDDeviceRef device,
    IOHIDReportType report_type,
    CFIndex report_id,
    uint8_t *report,
    CFIndex *report_length,
    CFTimeInterval timeout,
    IOHIDReportCallback callback,
    void *context);
IOReturn crosspuck_IOHIDDeviceSetValue(
    IOHIDDeviceRef device,
    IOHIDElementRef element,
    IOHIDValueRef value);
IOReturn crosspuck_IOHIDDeviceSetValueMultiple(
    IOHIDDeviceRef device,
    CFDictionaryRef multiple);
IOReturn crosspuck_IOHIDDeviceSetValueWithCallback(
    IOHIDDeviceRef device,
    IOHIDElementRef element,
    IOHIDValueRef value,
    CFTimeInterval timeout,
    IOHIDValueCallback callback,
    void *context);
IOReturn crosspuck_IOHIDDeviceSetValueMultipleWithCallback(
    IOHIDDeviceRef device,
    CFDictionaryRef multiple,
    CFTimeInterval timeout,
    IOHIDValueMultipleCallback callback,
    void *context);
void crosspuck_IOHIDDeviceRegisterInputReportCallback(
    IOHIDDeviceRef device,
    uint8_t *report,
    CFIndex report_length,
    IOHIDReportCallback callback,
    void *context);

static long long now_ms(void) {
    struct timeval tv;
    gettimeofday(&tv, NULL);
    return (long long)tv.tv_sec * 1000LL + tv.tv_usec / 1000LL;
}

static const char *report_type_label(IOHIDReportType report_type) {
    switch (report_type) {
    case kIOHIDReportTypeInput:
        return "input";
    case kIOHIDReportTypeOutput:
        return "output";
    case kIOHIDReportTypeFeature:
        return "feature";
    default:
        return "unknown";
    }
}

static long env_long(const char *name, long fallback) {
    const char *value = getenv(name);
    if (value == NULL || *value == '\0') {
        return fallback;
    }
    char *end = NULL;
    long parsed = strtol(value, &end, 0);
    return end != value ? parsed : fallback;
}

static bool env_bool(const char *name, bool fallback) {
    const char *value = getenv(name);
    if (value == NULL || *value == '\0') {
        return fallback;
    }
    return strcmp(value, "1") == 0 || strcasecmp(value, "true") == 0 ||
           strcasecmp(value, "yes") == 0 || strcasecmp(value, "on") == 0;
}

static void *resolve_iohid_symbol(const char *name, const void *replacement) {
    static void *iohid_handle = NULL;
    if (iohid_handle == NULL) {
        iohid_handle = dlopen(
            "/System/Library/Frameworks/IOKit.framework/IOKit",
            RTLD_LAZY | RTLD_LOCAL);
    }

    void *resolved = NULL;
    if (iohid_handle != NULL) {
        resolved = dlsym(iohid_handle, name);
    }
    if (resolved == NULL || resolved == replacement) {
        resolved = dlsym(RTLD_NEXT, name);
    }
    if (resolved == replacement) {
        return NULL;
    }
    return resolved;
}

typedef struct DeviceSnapshot {
    int vid;
    int pid;
    int usage_page;
    int usage;
    int location;
    char product[128];
    char serial[128];
} DeviceSnapshot;

static void json_escape(
    const char *source,
    char *buffer,
    size_t buffer_len) {
    if (buffer_len == 0) {
        return;
    }
    buffer[0] = '\0';
    if (source == NULL) {
        return;
    }

    size_t offset = 0;
    for (const unsigned char *cursor = (const unsigned char *)source;
         *cursor != '\0' && offset + 1 < buffer_len;
         cursor++) {
        const unsigned char ch = *cursor;
        const char *escape = NULL;
        switch (ch) {
        case '\\':
            escape = "\\\\";
            break;
        case '"':
            escape = "\\\"";
            break;
        case '\n':
            escape = "\\n";
            break;
        case '\r':
            escape = "\\r";
            break;
        case '\t':
            escape = "\\t";
            break;
        default:
            break;
        }

        if (escape != NULL) {
            size_t needed = strlen(escape);
            if (offset + needed >= buffer_len) {
                break;
            }
            memcpy(buffer + offset, escape, needed);
            offset += needed;
        } else if (ch < 0x20) {
            if (offset + 6 >= buffer_len) {
                break;
            }
            int written = snprintf(buffer + offset, buffer_len - offset,
                                   "\\u%04X", ch);
            if (written < 0) {
                break;
            }
            offset += (size_t)written;
        } else {
            buffer[offset++] = (char)ch;
        }
    }
    buffer[offset] = '\0';
}

static void log_json_event_prefix(const char *event) {
    fprintf(g_log_file,
            "{\"type\":\"hid_probe\",\"event\":\"%s\",\"unix_ms\":%lld,"
            "\"pid\":%d",
            event,
            now_ms(),
            getpid());
}

static void write_json_device(DeviceSnapshot *device) {
    char product[256];
    char serial[256];
    json_escape(device->product, product, sizeof(product));
    json_escape(device->serial, serial, sizeof(serial));
    fprintf(g_log_file,
            ",\"device\":{\"vid\":%d,\"pid\":%d,"
            "\"vid_hex\":\"0x%04X\",\"pid_hex\":\"0x%04X\","
            "\"usage_page\":%d,\"usage\":%d,"
            "\"usage_page_hex\":\"0x%04X\",\"usage_hex\":\"0x%04X\","
            "\"location\":\"0x%X\",\"product\":\"%s\",\"serial\":\"%s\"}",
            device->vid,
            device->pid,
            device->vid,
            device->pid,
            device->usage_page,
            device->usage,
            device->usage_page,
            device->usage,
            device->location,
            product,
            serial);
}

static void write_json_hex_field(const char *name, const char *hex) {
    char escaped[2048];
    json_escape(hex, escaped, sizeof(escaped));
    fprintf(g_log_file, ",\"%s\":\"%s\"", name, escaped);
}

static void flush_json_line(void) {
    fputs("}\n", g_log_file);
    fflush(g_log_file);
}

static void initialize_probe(void) {
    g_filter_vid = (int)env_long("CROSSPUCK_HOST_HID_VID", 0x28DE);
    g_filter_pid = (int)env_long("CROSSPUCK_HOST_HID_PID", 0x1304);
    g_log_all = env_bool("CROSSPUCK_HOST_HID_LOG_ALL", false);
    g_jsonl = env_bool("CROSSPUCK_HOST_HID_JSONL", false);
    g_log_get = env_bool("CROSSPUCK_HOST_HID_LOG_GET", true);
    g_log_input = env_bool("CROSSPUCK_HOST_HID_LOG_INPUT", true);
    g_log_value = env_bool("CROSSPUCK_HOST_HID_LOG_VALUE", true);
    long max_bytes = env_long("CROSSPUCK_HOST_HID_MAX_BYTES", 128);
    g_max_bytes = max_bytes > 0 ? (size_t)max_bytes : 128;

    const char *path = getenv("CROSSPUCK_HOST_HID_LOG");
    if (path == NULL || *path == '\0') {
        path = "/tmp/crosspuck-host-hid.log";
    }
    g_log_file = fopen(path, "a");
    if (g_log_file == NULL) {
        g_log_file = stderr;
    }

    real_IOHIDDeviceOpen =
        (IOHIDDeviceOpenFn)resolve_iohid_symbol(
            "IOHIDDeviceOpen", (const void *)crosspuck_IOHIDDeviceOpen);
    real_IOHIDDeviceGetReport =
        (IOHIDDeviceGetReportFn)resolve_iohid_symbol(
            "IOHIDDeviceGetReport", (const void *)crosspuck_IOHIDDeviceGetReport);
    real_IOHIDDeviceSetReport =
        (IOHIDDeviceSetReportFn)resolve_iohid_symbol(
            "IOHIDDeviceSetReport", (const void *)crosspuck_IOHIDDeviceSetReport);
    real_IOHIDDeviceSetReportWithCallback =
        (IOHIDDeviceSetReportWithCallbackFn)resolve_iohid_symbol(
            "IOHIDDeviceSetReportWithCallback",
            (const void *)crosspuck_IOHIDDeviceSetReportWithCallback);
    real_IOHIDDeviceGetReportWithCallback =
        (IOHIDDeviceGetReportWithCallbackFn)resolve_iohid_symbol(
            "IOHIDDeviceGetReportWithCallback",
            (const void *)crosspuck_IOHIDDeviceGetReportWithCallback);
    real_IOHIDDeviceSetValue =
        (IOHIDDeviceSetValueFn)resolve_iohid_symbol(
            "IOHIDDeviceSetValue", (const void *)crosspuck_IOHIDDeviceSetValue);
    real_IOHIDDeviceSetValueMultiple =
        (IOHIDDeviceSetValueMultipleFn)resolve_iohid_symbol(
            "IOHIDDeviceSetValueMultiple",
            (const void *)crosspuck_IOHIDDeviceSetValueMultiple);
    real_IOHIDDeviceSetValueWithCallback =
        (IOHIDDeviceSetValueWithCallbackFn)resolve_iohid_symbol(
            "IOHIDDeviceSetValueWithCallback",
            (const void *)crosspuck_IOHIDDeviceSetValueWithCallback);
    real_IOHIDDeviceSetValueMultipleWithCallback =
        (IOHIDDeviceSetValueMultipleWithCallbackFn)resolve_iohid_symbol(
            "IOHIDDeviceSetValueMultipleWithCallback",
            (const void *)crosspuck_IOHIDDeviceSetValueMultipleWithCallback);
    real_IOHIDDeviceRegisterInputReportCallback =
        (IOHIDDeviceRegisterInputReportCallbackFn)resolve_iohid_symbol(
            "IOHIDDeviceRegisterInputReportCallback",
            (const void *)crosspuck_IOHIDDeviceRegisterInputReportCallback);

    pthread_mutex_lock(&g_log_mutex);
    if (g_jsonl) {
        log_json_event_prefix("probe_loaded");
        fprintf(g_log_file,
                ",\"filter_vid\":%d,\"filter_pid\":%d,"
                "\"filter_vid_hex\":\"0x%04X\",\"filter_pid_hex\":\"0x%04X\","
                "\"log_all\":%s,\"log_get\":%s,\"log_input\":%s,"
                "\"log_value\":%s,\"max_bytes\":%zu",
                g_filter_vid,
                g_filter_pid,
                g_filter_vid,
                g_filter_pid,
                g_log_all ? "true" : "false",
                g_log_get ? "true" : "false",
                g_log_input ? "true" : "false",
                g_log_value ? "true" : "false",
                g_max_bytes);
        flush_json_line();
    } else {
        fprintf(g_log_file,
                "\n==== crosspuck host hid probe loaded pid=%d unix_ms=%lld "
                "filter_vid=0x%04X filter_pid=0x%04X log_all=%d "
                "log_get=%d log_input=%d log_value=%d max_bytes=%zu ====\n",
                getpid(), now_ms(), g_filter_vid, g_filter_pid, g_log_all,
                g_log_get, g_log_input, g_log_value, g_max_bytes);
    }
    fflush(g_log_file);
    pthread_mutex_unlock(&g_log_mutex);
}

static void initialize_once(void) {
    static pthread_once_t once = PTHREAD_ONCE_INIT;
    pthread_once(&once, initialize_probe);
}

__attribute__((constructor)) static void crosspuck_probe_constructor(void) {
    if (env_bool("CROSSPUCK_HOST_HID_LOG_LOAD", false)) {
        initialize_once();
    }
}

static int cf_number_property(IOHIDDeviceRef device, CFStringRef key, int fallback) {
    CFTypeRef value = IOHIDDeviceGetProperty(device, key);
    if (value == NULL || CFGetTypeID(value) != CFNumberGetTypeID()) {
        return fallback;
    }
    int out = fallback;
    CFNumberGetValue((CFNumberRef)value, kCFNumberIntType, &out);
    return out;
}

static void cf_string_property(
    IOHIDDeviceRef device,
    CFStringRef key,
    char *buffer,
    size_t buffer_len) {
    if (buffer_len == 0) {
        return;
    }
    buffer[0] = '\0';
    CFTypeRef value = IOHIDDeviceGetProperty(device, key);
    if (value == NULL || CFGetTypeID(value) != CFStringGetTypeID()) {
        return;
    }
    CFStringGetCString((CFStringRef)value, buffer, buffer_len, kCFStringEncodingUTF8);
}

static DeviceSnapshot snapshot_device(IOHIDDeviceRef device) {
    DeviceSnapshot snapshot;
    memset(&snapshot, 0, sizeof(snapshot));
    snapshot.vid = cf_number_property(device, CFSTR(kIOHIDVendorIDKey), -1);
    snapshot.pid = cf_number_property(device, CFSTR(kIOHIDProductIDKey), -1);
    snapshot.usage_page =
        cf_number_property(device, CFSTR(kIOHIDPrimaryUsagePageKey), -1);
    snapshot.usage = cf_number_property(device, CFSTR(kIOHIDPrimaryUsageKey), -1);
    snapshot.location = cf_number_property(device, CFSTR(kIOHIDLocationIDKey), -1);
    cf_string_property(
        device, CFSTR(kIOHIDProductKey), snapshot.product, sizeof(snapshot.product));
    cf_string_property(
        device, CFSTR(kIOHIDSerialNumberKey), snapshot.serial, sizeof(snapshot.serial));
    return snapshot;
}

static bool should_log_device(IOHIDDeviceRef device) {
    if (g_log_all) {
        return true;
    }
    int vid = cf_number_property(device, CFSTR(kIOHIDVendorIDKey), -1);
    int pid = cf_number_property(device, CFSTR(kIOHIDProductIDKey), -1);
    return vid == g_filter_vid && pid == g_filter_pid;
}

static void describe_device(
    IOHIDDeviceRef device,
    char *buffer,
    size_t buffer_len) {
    int vid = cf_number_property(device, CFSTR(kIOHIDVendorIDKey), -1);
    int pid = cf_number_property(device, CFSTR(kIOHIDProductIDKey), -1);
    int usage_page = cf_number_property(device, CFSTR(kIOHIDPrimaryUsagePageKey), -1);
    int usage = cf_number_property(device, CFSTR(kIOHIDPrimaryUsageKey), -1);
    int location = cf_number_property(device, CFSTR(kIOHIDLocationIDKey), -1);
    char product[128];
    char serial[128];
    cf_string_property(device, CFSTR(kIOHIDProductKey), product, sizeof(product));
    cf_string_property(device, CFSTR(kIOHIDSerialNumberKey), serial, sizeof(serial));
    snprintf(buffer, buffer_len,
             "device=%p vid=0x%04X pid=0x%04X usage_page=0x%04X usage=0x%04X "
             "location=0x%X product=\"%s\" serial=\"%s\"",
             device, vid, pid, usage_page, usage, location, product, serial);
}

static void hex_bytes(
    const uint8_t *bytes,
    CFIndex len,
    char *buffer,
    size_t buffer_len) {
    if (buffer_len == 0) {
        return;
    }
    buffer[0] = '\0';
    if (bytes == NULL || len <= 0) {
        snprintf(buffer, buffer_len, "-");
        return;
    }

    size_t limit = (size_t)len < g_max_bytes ? (size_t)len : g_max_bytes;
    size_t offset = 0;
    for (size_t i = 0; i < limit && offset + 4 < buffer_len; i++) {
        int written = snprintf(buffer + offset, buffer_len - offset, "%s%02X",
                               i == 0 ? "" : " ", bytes[i]);
        if (written < 0) {
            break;
        }
        offset += (size_t)written;
    }
    if ((size_t)len > limit && offset + 32 < buffer_len) {
        snprintf(buffer + offset, buffer_len - offset, " ...(+%zu bytes)",
                 (size_t)len - limit);
    }
}

static void log_line(const char *format, ...) {
    initialize_once();
    pthread_mutex_lock(&g_log_mutex);
    fprintf(g_log_file, "[%lld] ", now_ms());
    va_list args;
    va_start(args, format);
    vfprintf(g_log_file, format, args);
    va_end(args);
    fputc('\n', g_log_file);
    fflush(g_log_file);
    pthread_mutex_unlock(&g_log_mutex);
}

static void log_open_json(
    IOReturn result,
    IOOptionBits options,
    IOHIDDeviceRef device) {
    DeviceSnapshot snapshot = snapshot_device(device);
    pthread_mutex_lock(&g_log_mutex);
    log_json_event_prefix("device_open");
    fprintf(g_log_file,
            ",\"result\":\"0x%08X\",\"options\":\"0x%X\"",
            result,
            options);
    write_json_device(&snapshot);
    flush_json_line();
    pthread_mutex_unlock(&g_log_mutex);
}

static void log_set_report_json(
    const char *event,
    const char *phase,
    IOReturn result,
    IOHIDDeviceRef device,
    IOHIDReportType report_type,
    CFIndex report_id,
    const uint8_t *report,
    CFIndex report_length,
    bool include_result,
    CFTimeInterval timeout,
    void *callback,
    void *context) {
    DeviceSnapshot snapshot = snapshot_device(device);
    char hex[1024];
    hex_bytes(report, report_length, hex, sizeof(hex));
    pthread_mutex_lock(&g_log_mutex);
    log_json_event_prefix(event);
    fprintf(g_log_file,
            ",\"phase\":\"%s\",\"direction\":\"host_to_device\","
            "\"report_type\":\"%s\",\"report_type_code\":%d,"
            "\"report_id\":%ld,\"report_id_hex\":\"0x%02lX\","
            "\"len\":%ld,\"timeout_ms\":%.3f,"
            "\"callback\":\"%p\",\"context\":\"%p\"",
            phase,
            report_type_label(report_type),
            report_type,
            report_id,
            report_id,
            report_length,
            timeout,
            callback,
            context);
    if (include_result) {
        fprintf(g_log_file, ",\"result\":\"0x%08X\"", result);
    }
    write_json_hex_field("hex", hex);
    write_json_device(&snapshot);
    flush_json_line();
    pthread_mutex_unlock(&g_log_mutex);
}

static void log_get_report_json(
    const char *event,
    const char *phase,
    IOReturn result,
    IOHIDDeviceRef device,
    IOHIDReportType report_type,
    CFIndex report_id,
    const uint8_t *report,
    CFIndex report_length,
    CFIndex requested_length,
    bool include_result,
    CFTimeInterval timeout,
    void *callback,
    void *context) {
    DeviceSnapshot snapshot = snapshot_device(device);
    char hex[1024];
    hex_bytes(report, report_length, hex, sizeof(hex));
    pthread_mutex_lock(&g_log_mutex);
    log_json_event_prefix(event);
    fprintf(g_log_file,
            ",\"phase\":\"%s\",\"direction\":\"%s\","
            "\"report_type\":\"%s\",\"report_type_code\":%d,"
            "\"report_id\":%ld,\"report_id_hex\":\"0x%02lX\","
            "\"requested_len\":%ld,\"len\":%ld,\"timeout_ms\":%.3f,"
            "\"callback\":\"%p\",\"context\":\"%p\"",
            phase,
            strcmp(phase, "request") == 0 ? "host_to_device" : "device_to_host",
            report_type_label(report_type),
            report_type,
            report_id,
            report_id,
            requested_length,
            report_length,
            timeout,
            callback,
            context);
    if (include_result) {
        fprintf(g_log_file, ",\"result\":\"0x%08X\"", result);
    }
    write_json_hex_field("hex", hex);
    write_json_device(&snapshot);
    flush_json_line();
    pthread_mutex_unlock(&g_log_mutex);
}

static void log_input_json(
    IOReturn result,
    IOHIDDeviceRef device,
    IOHIDReportType report_type,
    uint32_t report_id,
    const uint8_t *report,
    CFIndex report_length) {
    DeviceSnapshot snapshot = snapshot_device(device);
    char hex[1024];
    hex_bytes(report, report_length, hex, sizeof(hex));
    pthread_mutex_lock(&g_log_mutex);
    log_json_event_prefix("input_report");
    fprintf(g_log_file,
            ",\"direction\":\"device_to_host\",\"result\":\"0x%08X\","
            "\"report_type\":\"%s\",\"report_type_code\":%d,"
            "\"report_id\":%u,\"report_id_hex\":\"0x%02X\",\"len\":%ld",
            result,
            report_type_label(report_type),
            report_type,
            report_id,
            report_id,
            report_length);
    write_json_hex_field("hex", hex);
    write_json_device(&snapshot);
    flush_json_line();
    pthread_mutex_unlock(&g_log_mutex);
}

static void write_json_element(IOHIDElementRef element) {
    if (element == NULL) {
        fprintf(g_log_file, ",\"element\":null");
        return;
    }
    fprintf(g_log_file,
            ",\"element\":{\"type\":%d,\"usage_page\":%u,\"usage\":%u,"
            "\"usage_page_hex\":\"0x%04X\",\"usage_hex\":\"0x%04X\","
            "\"report_id\":%u,\"report_id_hex\":\"0x%02X\","
            "\"report_size\":%u,\"report_count\":%u}",
            IOHIDElementGetType(element),
            IOHIDElementGetUsagePage(element),
            IOHIDElementGetUsage(element),
            IOHIDElementGetUsagePage(element),
            IOHIDElementGetUsage(element),
            IOHIDElementGetReportID(element),
            IOHIDElementGetReportID(element),
            IOHIDElementGetReportSize(element),
            IOHIDElementGetReportCount(element));
}

static void log_set_value_json(
    const char *event,
    const char *phase,
    IOReturn result,
    IOHIDDeviceRef device,
    IOHIDElementRef element,
    IOHIDValueRef value,
    bool include_result,
    CFTimeInterval timeout,
    void *callback,
    void *context,
    int multiple_index) {
    DeviceSnapshot snapshot = snapshot_device(device);
    CFIndex value_len = value != NULL ? IOHIDValueGetLength(value) : 0;
    CFIndex integer_value = value != NULL ? IOHIDValueGetIntegerValue(value) : 0;
    const uint8_t *value_bytes =
        value != NULL ? IOHIDValueGetBytePtr(value) : NULL;
    char hex[1024];
    hex_bytes(value_bytes, value_len, hex, sizeof(hex));

    pthread_mutex_lock(&g_log_mutex);
    log_json_event_prefix(event);
    fprintf(g_log_file,
            ",\"phase\":\"%s\",\"direction\":\"host_to_device\","
            "\"multiple_index\":%d,\"value_len\":%ld,"
            "\"integer_value\":%ld,\"timeout_ms\":%.3f,"
            "\"callback\":\"%p\",\"context\":\"%p\"",
            phase,
            multiple_index,
            value_len,
            integer_value,
            timeout,
            callback,
            context);
    if (include_result) {
        fprintf(g_log_file, ",\"result\":\"0x%08X\"", result);
    }
    write_json_hex_field("hex", hex);
    write_json_element(element);
    write_json_device(&snapshot);
    flush_json_line();
    pthread_mutex_unlock(&g_log_mutex);
}

typedef struct MultipleValueLogContext {
    IOHIDDeviceRef device;
    const char *event;
    const char *phase;
    CFTimeInterval timeout;
    void *callback;
    void *context;
    int index;
} MultipleValueLogContext;

static void log_multiple_value_item(
    const void *key,
    const void *value,
    void *context) {
    MultipleValueLogContext *state = (MultipleValueLogContext *)context;
    state->index += 1;
    log_set_value_json(
        state->event,
        state->phase,
        kIOReturnSuccess,
        state->device,
        (IOHIDElementRef)key,
        (IOHIDValueRef)value,
        false,
        state->timeout,
        state->callback,
        state->context,
        state->index);
}

IOReturn crosspuck_IOHIDDeviceOpen(IOHIDDeviceRef device, IOOptionBits options) {
    initialize_once();
    IOReturn result = real_IOHIDDeviceOpen != NULL
                          ? real_IOHIDDeviceOpen(device, options)
                          : kIOReturnUnsupported;
    if (should_log_device(device)) {
        if (g_jsonl) {
            log_open_json(result, options, device);
        } else {
            char device_text[512];
            describe_device(device, device_text, sizeof(device_text));
            log_line("IOHIDDeviceOpen result=0x%08X options=0x%X %s", result,
                     options, device_text);
        }
    }
    return result;
}

IOReturn crosspuck_IOHIDDeviceSetReport(
    IOHIDDeviceRef device,
    IOHIDReportType report_type,
    CFIndex report_id,
    const uint8_t *report,
    CFIndex report_length) {
    initialize_once();
    if (should_log_device(device)) {
        if (g_jsonl) {
            log_set_report_json(
                "set_report",
                "request",
                kIOReturnSuccess,
                device,
                report_type,
                report_id,
                report,
                report_length,
                false,
                0.0,
                NULL,
                NULL);
        } else {
            char device_text[512];
            char hex[1024];
            describe_device(device, device_text, sizeof(device_text));
            hex_bytes(report, report_length, hex, sizeof(hex));
            log_line("SET type=%s(%d) report_id=0x%02lX len=%ld bytes=%s %s",
                     report_type_label(report_type), report_type, report_id,
                     report_length, hex, device_text);
        }
    }
    IOReturn result = real_IOHIDDeviceSetReport != NULL
                          ? real_IOHIDDeviceSetReport(device, report_type, report_id,
                                                       report, report_length)
                          : kIOReturnUnsupported;
    if (should_log_device(device)) {
        if (g_jsonl) {
            log_set_report_json(
                "set_report",
                "result",
                result,
                device,
                report_type,
                report_id,
                report,
                report_length,
                true,
                0.0,
                NULL,
                NULL);
        } else {
            log_line("SET result=0x%08X type=%s(%d) report_id=0x%02lX len=%ld",
                     result, report_type_label(report_type), report_type, report_id,
                     report_length);
        }
    }
    return result;
}

IOReturn crosspuck_IOHIDDeviceGetReport(
    IOHIDDeviceRef device,
    IOHIDReportType report_type,
    CFIndex report_id,
    uint8_t *report,
    CFIndex *report_length) {
    initialize_once();
    CFIndex requested_length = report_length != NULL ? *report_length : 0;
    if (g_log_get && should_log_device(device)) {
        if (g_jsonl) {
            log_get_report_json(
                "get_report",
                "request",
                kIOReturnSuccess,
                device,
                report_type,
                report_id,
                report,
                0,
                requested_length,
                false,
                0.0,
                NULL,
                NULL);
        } else {
            char device_text[512];
            describe_device(device, device_text, sizeof(device_text));
            log_line(
                "GET request type=%s(%d) report_id=0x%02lX requested_len=%ld %s",
                report_type_label(report_type), report_type, report_id,
                requested_length, device_text);
        }
    }
    IOReturn result = real_IOHIDDeviceGetReport != NULL
                          ? real_IOHIDDeviceGetReport(device, report_type, report_id,
                                                       report, report_length)
                          : kIOReturnUnsupported;
    if (g_log_get && should_log_device(device)) {
        CFIndex returned_length = report_length != NULL ? *report_length : 0;
        if (g_jsonl) {
            log_get_report_json(
                "get_report",
                "result",
                result,
                device,
                report_type,
                report_id,
                report,
                returned_length,
                requested_length,
                true,
                0.0,
                NULL,
                NULL);
        } else {
            char hex[1024];
            hex_bytes(report, returned_length, hex, sizeof(hex));
            log_line(
                "GET result=0x%08X type=%s(%d) report_id=0x%02lX len=%ld bytes=%s",
                result, report_type_label(report_type), report_type, report_id,
                returned_length, hex);
        }
    }
    return result;
}

IOReturn crosspuck_IOHIDDeviceSetReportWithCallback(
    IOHIDDeviceRef device,
    IOHIDReportType report_type,
    CFIndex report_id,
    const uint8_t *report,
    CFIndex report_length,
    CFTimeInterval timeout,
    IOHIDReportCallback callback,
    void *context) {
    initialize_once();
    if (should_log_device(device)) {
        if (g_jsonl) {
            log_set_report_json(
                "set_report_callback",
                "request",
                kIOReturnSuccess,
                device,
                report_type,
                report_id,
                report,
                report_length,
                false,
                timeout,
                (void *)callback,
                context);
        } else {
            char device_text[512];
            char hex[1024];
            describe_device(device, device_text, sizeof(device_text));
            hex_bytes(report, report_length, hex, sizeof(hex));
            log_line(
                "SET-CB type=%s(%d) report_id=0x%02lX len=%ld timeout=%.3f "
                "callback=%p context=%p bytes=%s %s",
                report_type_label(report_type), report_type, report_id,
                report_length, timeout, callback, context, hex, device_text);
        }
    }

    IOReturn result = real_IOHIDDeviceSetReportWithCallback != NULL
                          ? real_IOHIDDeviceSetReportWithCallback(
                                device, report_type, report_id, report, report_length,
                                timeout, callback, context)
                          : kIOReturnUnsupported;
    if (should_log_device(device)) {
        if (g_jsonl) {
            log_set_report_json(
                "set_report_callback",
                "result",
                result,
                device,
                report_type,
                report_id,
                report,
                report_length,
                true,
                timeout,
                (void *)callback,
                context);
        } else {
            log_line(
                "SET-CB result=0x%08X type=%s(%d) report_id=0x%02lX len=%ld",
                result, report_type_label(report_type), report_type, report_id,
                report_length);
        }
    }
    return result;
}

IOReturn crosspuck_IOHIDDeviceGetReportWithCallback(
    IOHIDDeviceRef device,
    IOHIDReportType report_type,
    CFIndex report_id,
    uint8_t *report,
    CFIndex *report_length,
    CFTimeInterval timeout,
    IOHIDReportCallback callback,
    void *context) {
    initialize_once();
    CFIndex requested_length = report_length != NULL ? *report_length : 0;
    if (g_log_get && should_log_device(device)) {
        if (g_jsonl) {
            log_get_report_json(
                "get_report_callback",
                "request",
                kIOReturnSuccess,
                device,
                report_type,
                report_id,
                report,
                0,
                requested_length,
                false,
                timeout,
                (void *)callback,
                context);
        } else {
            char device_text[512];
            describe_device(device, device_text, sizeof(device_text));
            log_line(
                "GET-CB request type=%s(%d) report_id=0x%02lX requested_len=%ld "
                "timeout=%.3f callback=%p context=%p %s",
                report_type_label(report_type), report_type, report_id,
                requested_length, timeout, callback, context, device_text);
        }
    }

    IOReturn result = real_IOHIDDeviceGetReportWithCallback != NULL
                          ? real_IOHIDDeviceGetReportWithCallback(
                                device, report_type, report_id, report, report_length,
                                timeout, callback, context)
                          : kIOReturnUnsupported;
    if (g_log_get && should_log_device(device)) {
        CFIndex returned_length = report_length != NULL ? *report_length : 0;
        if (g_jsonl) {
            log_get_report_json(
                "get_report_callback",
                "result",
                result,
                device,
                report_type,
                report_id,
                report,
                returned_length,
                requested_length,
                true,
                timeout,
                (void *)callback,
                context);
        } else {
            char hex[1024];
            hex_bytes(report, returned_length, hex, sizeof(hex));
            log_line(
                "GET-CB result=0x%08X type=%s(%d) report_id=0x%02lX "
                "len=%ld bytes=%s",
                result, report_type_label(report_type), report_type, report_id,
                returned_length, hex);
        }
    }
    return result;
}

IOReturn crosspuck_IOHIDDeviceSetValue(
    IOHIDDeviceRef device,
    IOHIDElementRef element,
    IOHIDValueRef value) {
    initialize_once();
    if (g_log_value && should_log_device(device)) {
        if (g_jsonl) {
            log_set_value_json(
                "set_value",
                "request",
                kIOReturnSuccess,
                device,
                element,
                value,
                false,
                0.0,
                NULL,
                NULL,
                0);
        } else {
            char device_text[512];
            char hex[1024];
            const uint8_t *value_bytes =
                value != NULL ? IOHIDValueGetBytePtr(value) : NULL;
            CFIndex value_len = value != NULL ? IOHIDValueGetLength(value) : 0;
            describe_device(device, device_text, sizeof(device_text));
            hex_bytes(value_bytes, value_len, hex, sizeof(hex));
            log_line(
                "SET-VALUE usage_page=0x%04X usage=0x%04X report_id=0x%02X "
                "value_len=%ld integer=%ld bytes=%s %s",
                element != NULL ? IOHIDElementGetUsagePage(element) : 0,
                element != NULL ? IOHIDElementGetUsage(element) : 0,
                element != NULL ? IOHIDElementGetReportID(element) : 0,
                value_len,
                value != NULL ? IOHIDValueGetIntegerValue(value) : 0,
                hex,
                device_text);
        }
    }
    IOReturn result = real_IOHIDDeviceSetValue != NULL
                          ? real_IOHIDDeviceSetValue(device, element, value)
                          : kIOReturnUnsupported;
    if (g_log_value && should_log_device(device)) {
        if (g_jsonl) {
            log_set_value_json(
                "set_value",
                "result",
                result,
                device,
                element,
                value,
                true,
                0.0,
                NULL,
                NULL,
                0);
        } else {
            log_line("SET-VALUE result=0x%08X", result);
        }
    }
    return result;
}

IOReturn crosspuck_IOHIDDeviceSetValueMultiple(
    IOHIDDeviceRef device,
    CFDictionaryRef multiple) {
    initialize_once();
    CFIndex count = multiple != NULL ? CFDictionaryGetCount(multiple) : 0;
    if (g_log_value && should_log_device(device)) {
        if (g_jsonl && multiple != NULL) {
            MultipleValueLogContext state = {
                .device = device,
                .event = "set_value_multiple_item",
                .phase = "request",
                .timeout = 0.0,
                .callback = NULL,
                .context = NULL,
                .index = 0,
            };
            CFDictionaryApplyFunction(multiple, log_multiple_value_item, &state);
        } else if (!g_jsonl) {
            char device_text[512];
            describe_device(device, device_text, sizeof(device_text));
            log_line("SET-VALUE-MULTIPLE count=%ld %s", count, device_text);
        }
    }
    IOReturn result = real_IOHIDDeviceSetValueMultiple != NULL
                          ? real_IOHIDDeviceSetValueMultiple(device, multiple)
                          : kIOReturnUnsupported;
    if (g_log_value && should_log_device(device)) {
        if (g_jsonl) {
            DeviceSnapshot snapshot = snapshot_device(device);
            pthread_mutex_lock(&g_log_mutex);
            log_json_event_prefix("set_value_multiple");
            fprintf(g_log_file,
                    ",\"phase\":\"result\",\"direction\":\"host_to_device\","
                    "\"count\":%ld,\"result\":\"0x%08X\"",
                    count,
                    result);
            write_json_device(&snapshot);
            flush_json_line();
            pthread_mutex_unlock(&g_log_mutex);
        } else {
            log_line("SET-VALUE-MULTIPLE result=0x%08X count=%ld", result, count);
        }
    }
    return result;
}

IOReturn crosspuck_IOHIDDeviceSetValueWithCallback(
    IOHIDDeviceRef device,
    IOHIDElementRef element,
    IOHIDValueRef value,
    CFTimeInterval timeout,
    IOHIDValueCallback callback,
    void *context) {
    initialize_once();
    if (g_log_value && should_log_device(device)) {
        if (g_jsonl) {
            log_set_value_json(
                "set_value_callback",
                "request",
                kIOReturnSuccess,
                device,
                element,
                value,
                false,
                timeout,
                (void *)callback,
                context,
                0);
        } else {
            char device_text[512];
            describe_device(device, device_text, sizeof(device_text));
            log_line(
                "SET-VALUE-CB usage_page=0x%04X usage=0x%04X report_id=0x%02X "
                "timeout=%.3f callback=%p context=%p %s",
                element != NULL ? IOHIDElementGetUsagePage(element) : 0,
                element != NULL ? IOHIDElementGetUsage(element) : 0,
                element != NULL ? IOHIDElementGetReportID(element) : 0,
                timeout,
                callback,
                context,
                device_text);
        }
    }
    IOReturn result = real_IOHIDDeviceSetValueWithCallback != NULL
                          ? real_IOHIDDeviceSetValueWithCallback(
                                device, element, value, timeout, callback, context)
                          : kIOReturnUnsupported;
    if (g_log_value && should_log_device(device)) {
        if (g_jsonl) {
            log_set_value_json(
                "set_value_callback",
                "result",
                result,
                device,
                element,
                value,
                true,
                timeout,
                (void *)callback,
                context,
                0);
        } else {
            log_line("SET-VALUE-CB result=0x%08X", result);
        }
    }
    return result;
}

IOReturn crosspuck_IOHIDDeviceSetValueMultipleWithCallback(
    IOHIDDeviceRef device,
    CFDictionaryRef multiple,
    CFTimeInterval timeout,
    IOHIDValueMultipleCallback callback,
    void *context) {
    initialize_once();
    CFIndex count = multiple != NULL ? CFDictionaryGetCount(multiple) : 0;
    if (g_log_value && should_log_device(device)) {
        if (g_jsonl && multiple != NULL) {
            MultipleValueLogContext state = {
                .device = device,
                .event = "set_value_multiple_callback_item",
                .phase = "request",
                .timeout = timeout,
                .callback = (void *)callback,
                .context = context,
                .index = 0,
            };
            CFDictionaryApplyFunction(multiple, log_multiple_value_item, &state);
        } else if (!g_jsonl) {
            char device_text[512];
            describe_device(device, device_text, sizeof(device_text));
            log_line(
                "SET-VALUE-MULTIPLE-CB count=%ld timeout=%.3f callback=%p "
                "context=%p %s",
                count,
                timeout,
                callback,
                context,
                device_text);
        }
    }
    IOReturn result = real_IOHIDDeviceSetValueMultipleWithCallback != NULL
                          ? real_IOHIDDeviceSetValueMultipleWithCallback(
                                device, multiple, timeout, callback, context)
                          : kIOReturnUnsupported;
    if (g_log_value && should_log_device(device)) {
        if (g_jsonl) {
            DeviceSnapshot snapshot = snapshot_device(device);
            pthread_mutex_lock(&g_log_mutex);
            log_json_event_prefix("set_value_multiple_callback");
            fprintf(g_log_file,
                    ",\"phase\":\"result\",\"direction\":\"host_to_device\","
                    "\"count\":%ld,\"timeout_ms\":%.3f,\"callback\":\"%p\","
                    "\"context\":\"%p\",\"result\":\"0x%08X\"",
                    count,
                    timeout,
                    callback,
                    context,
                    result);
            write_json_device(&snapshot);
            flush_json_line();
            pthread_mutex_unlock(&g_log_mutex);
        } else {
            log_line("SET-VALUE-MULTIPLE-CB result=0x%08X count=%ld", result, count);
        }
    }
    return result;
}

static void crosspuck_input_report_callback(
    void *context,
    IOReturn result,
    void *sender,
    IOHIDReportType report_type,
    uint32_t report_id,
    uint8_t *report,
    CFIndex report_length) {
    InputReportCallbackContext *wrapped =
        (InputReportCallbackContext *)context;
    IOHIDDeviceRef device = wrapped != NULL && wrapped->device != NULL
                                ? wrapped->device
                                : (IOHIDDeviceRef)sender;

    initialize_once();
    if (g_log_input && device != NULL && should_log_device(device)) {
        if (g_jsonl) {
            log_input_json(result, device, report_type, report_id, report, report_length);
        } else {
            char device_text[512];
            char hex[1024];
            describe_device(device, device_text, sizeof(device_text));
            hex_bytes(report, report_length, hex, sizeof(hex));
            log_line(
                "INPUT callback result=0x%08X type=%s(%d) report_id=0x%02X "
                "len=%ld bytes=%s %s",
                result, report_type_label(report_type), report_type, report_id,
                report_length, hex, device_text);
        }
    }

    if (wrapped != NULL && wrapped->callback != NULL) {
        wrapped->callback(
            wrapped->context,
            result,
            sender,
            report_type,
            report_id,
            report,
            report_length);
    }
}

void crosspuck_IOHIDDeviceRegisterInputReportCallback(
    IOHIDDeviceRef device,
    uint8_t *report,
    CFIndex report_length,
    IOHIDReportCallback callback,
    void *context) {
    initialize_once();

    if (g_log_input && should_log_device(device)) {
        if (g_jsonl) {
            DeviceSnapshot snapshot = snapshot_device(device);
            pthread_mutex_lock(&g_log_mutex);
            log_json_event_prefix("register_input_report_callback");
            fprintf(g_log_file,
                    ",\"direction\":\"device_to_host\",\"report_buffer\":\"%p\","
                    "\"len\":%ld,\"callback\":\"%p\",\"context\":\"%p\"",
                    report,
                    report_length,
                    callback,
                    context);
            write_json_device(&snapshot);
            flush_json_line();
            pthread_mutex_unlock(&g_log_mutex);
        } else {
            char device_text[512];
            describe_device(device, device_text, sizeof(device_text));
            log_line(
                "REGISTER input_report_callback report_buffer=%p report_len=%ld "
                "callback=%p context=%p %s",
                report,
                report_length,
                callback,
                context,
                device_text);
        }
    }

    InputReportCallbackContext *wrapped =
        (InputReportCallbackContext *)calloc(1, sizeof(*wrapped));
    if (callback == NULL || wrapped == NULL) {
        if (real_IOHIDDeviceRegisterInputReportCallback != NULL) {
            real_IOHIDDeviceRegisterInputReportCallback(
                device, report, report_length, callback, context);
        }
        free(wrapped);
        return;
    }

    wrapped->device = device;
    wrapped->callback = callback;
    wrapped->context = context;

    if (real_IOHIDDeviceRegisterInputReportCallback != NULL) {
        real_IOHIDDeviceRegisterInputReportCallback(
            device,
            report,
            report_length,
            crosspuck_input_report_callback,
            wrapped);
    }
}

INTERPOSE(crosspuck_IOHIDDeviceOpen, IOHIDDeviceOpen);
INTERPOSE(crosspuck_IOHIDDeviceGetReport, IOHIDDeviceGetReport);
INTERPOSE(crosspuck_IOHIDDeviceSetReport, IOHIDDeviceSetReport);
INTERPOSE(crosspuck_IOHIDDeviceGetReportWithCallback, IOHIDDeviceGetReportWithCallback);
INTERPOSE(crosspuck_IOHIDDeviceSetReportWithCallback, IOHIDDeviceSetReportWithCallback);
INTERPOSE(crosspuck_IOHIDDeviceSetValue, IOHIDDeviceSetValue);
INTERPOSE(crosspuck_IOHIDDeviceSetValueMultiple, IOHIDDeviceSetValueMultiple);
INTERPOSE(crosspuck_IOHIDDeviceSetValueWithCallback, IOHIDDeviceSetValueWithCallback);
INTERPOSE(
    crosspuck_IOHIDDeviceSetValueMultipleWithCallback,
    IOHIDDeviceSetValueMultipleWithCallback);
INTERPOSE(
    crosspuck_IOHIDDeviceRegisterInputReportCallback,
    IOHIDDeviceRegisterInputReportCallback);
