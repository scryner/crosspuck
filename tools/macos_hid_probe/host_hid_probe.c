#include <CoreFoundation/CoreFoundation.h>
#include <IOKit/hid/IOHIDKeys.h>
#include <IOKit/hid/IOHIDLib.h>
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
static size_t g_max_bytes = 128;

static IOHIDDeviceOpenFn real_IOHIDDeviceOpen = NULL;
static IOHIDDeviceGetReportFn real_IOHIDDeviceGetReport = NULL;
static IOHIDDeviceSetReportFn real_IOHIDDeviceSetReport = NULL;
static IOHIDDeviceRegisterInputReportCallbackFn
    real_IOHIDDeviceRegisterInputReportCallback = NULL;

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

static void initialize_probe(void) {
    g_filter_vid = (int)env_long("CROSSPUCK_HOST_HID_VID", 0x28DE);
    g_filter_pid = (int)env_long("CROSSPUCK_HOST_HID_PID", 0x1304);
    g_log_all = env_bool("CROSSPUCK_HOST_HID_LOG_ALL", false);
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
        (IOHIDDeviceOpenFn)dlsym(RTLD_NEXT, "IOHIDDeviceOpen");
    real_IOHIDDeviceGetReport =
        (IOHIDDeviceGetReportFn)dlsym(RTLD_NEXT, "IOHIDDeviceGetReport");
    real_IOHIDDeviceSetReport =
        (IOHIDDeviceSetReportFn)dlsym(RTLD_NEXT, "IOHIDDeviceSetReport");
    real_IOHIDDeviceRegisterInputReportCallback =
        (IOHIDDeviceRegisterInputReportCallbackFn)dlsym(
            RTLD_NEXT, "IOHIDDeviceRegisterInputReportCallback");

    pthread_mutex_lock(&g_log_mutex);
    fprintf(g_log_file,
            "\n==== crosspuck host hid probe loaded pid=%d unix_ms=%lld "
            "filter_vid=0x%04X filter_pid=0x%04X log_all=%d max_bytes=%zu ====\n",
            getpid(), now_ms(), g_filter_vid, g_filter_pid, g_log_all,
            g_max_bytes);
    fflush(g_log_file);
    pthread_mutex_unlock(&g_log_mutex);
}

static void initialize_once(void) {
    static pthread_once_t once = PTHREAD_ONCE_INIT;
    pthread_once(&once, initialize_probe);
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

IOReturn crosspuck_IOHIDDeviceOpen(IOHIDDeviceRef device, IOOptionBits options) {
    initialize_once();
    IOReturn result = real_IOHIDDeviceOpen != NULL
                          ? real_IOHIDDeviceOpen(device, options)
                          : kIOReturnUnsupported;
    if (should_log_device(device)) {
        char device_text[512];
        describe_device(device, device_text, sizeof(device_text));
        log_line("IOHIDDeviceOpen result=0x%08X options=0x%X %s", result, options,
                 device_text);
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
        char device_text[512];
        char hex[1024];
        describe_device(device, device_text, sizeof(device_text));
        hex_bytes(report, report_length, hex, sizeof(hex));
        log_line("SET type=%s(%d) report_id=0x%02lX len=%ld bytes=%s %s",
                 report_type_label(report_type), report_type, report_id, report_length,
                 hex, device_text);
    }
    IOReturn result = real_IOHIDDeviceSetReport != NULL
                          ? real_IOHIDDeviceSetReport(device, report_type, report_id,
                                                       report, report_length)
                          : kIOReturnUnsupported;
    if (should_log_device(device)) {
        log_line("SET result=0x%08X type=%s(%d) report_id=0x%02lX len=%ld", result,
                 report_type_label(report_type), report_type, report_id, report_length);
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
    if (should_log_device(device)) {
        char device_text[512];
        describe_device(device, device_text, sizeof(device_text));
        log_line("GET request type=%s(%d) report_id=0x%02lX requested_len=%ld %s",
                 report_type_label(report_type), report_type, report_id, requested_length,
                 device_text);
    }
    IOReturn result = real_IOHIDDeviceGetReport != NULL
                          ? real_IOHIDDeviceGetReport(device, report_type, report_id,
                                                       report, report_length)
                          : kIOReturnUnsupported;
    if (should_log_device(device)) {
        CFIndex returned_length = report_length != NULL ? *report_length : 0;
        char hex[1024];
        hex_bytes(report, returned_length, hex, sizeof(hex));
        log_line("GET result=0x%08X type=%s(%d) report_id=0x%02lX len=%ld bytes=%s",
                 result, report_type_label(report_type), report_type, report_id,
                 returned_length, hex);
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
    if (device != NULL && should_log_device(device)) {
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

    if (should_log_device(device)) {
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

    InputReportCallbackContext *wrapped =
        (InputReportCallbackContext *)calloc(1, sizeof(*wrapped));
    if (wrapped == NULL) {
        if (real_IOHIDDeviceRegisterInputReportCallback != NULL) {
            real_IOHIDDeviceRegisterInputReportCallback(
                device, report, report_length, callback, context);
        }
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
INTERPOSE(
    crosspuck_IOHIDDeviceRegisterInputReportCallback,
    IOHIDDeviceRegisterInputReportCallback);
