// AVFoundation and Core Media names the app's recording and playback import, which
// arrived in 10.7. 10.6 has none of their classes, so the app never reaches these: it
// opens recordings in the system's player and says recording needs 10.7.
#include <CoreFoundation/CoreFoundation.h>
#include <stddef.h>

const CFStringRef AVFormatIDKey = CFSTR("AVFormatIDKey");
const CFStringRef AVSampleRateKey = CFSTR("AVSampleRateKey");
const CFStringRef AVNumberOfChannelsKey = CFSTR("AVNumberOfChannelsKey");
const CFStringRef AVLinearPCMBitDepthKey = CFSTR("AVLinearPCMBitDepthKey");
const CFStringRef AVLinearPCMIsFloatKey = CFSTR("AVLinearPCMIsFloatKey");
const CFStringRef AVLinearPCMIsBigEndianKey = CFSTR("AVLinearPCMIsBigEndianKey");
const CFStringRef AVLinearPCMIsNonInterleaved = CFSTR("AVLinearPCMIsNonInterleaved");
const CFStringRef AVMediaTypeAudio = CFSTR("soun");
const CFStringRef AVMediaTypeVideo = CFSTR("vide");
const CFStringRef AVCaptureSessionPreset320x240 = CFSTR("AVCaptureSessionPreset320x240");

typedef struct {
    long long value;
    int timescale;
    unsigned flags;
    long long epoch;
} CMTime;

void *CMSampleBufferGetImageBuffer(void *sample) { return NULL; }
void *CMSampleBufferGetDataBuffer(void *sample) { return NULL; }
size_t CMBlockBufferGetDataLength(void *buffer) { return 0; }
int CMBlockBufferCopyDataBytes(void *buffer, size_t offset, size_t length, void *destination) {
    return -1;
}
CMTime CMSampleBufferGetPresentationTimeStamp(void *sample) {
    CMTime invalid = {0, 0, 0, 0};
    return invalid;
}
