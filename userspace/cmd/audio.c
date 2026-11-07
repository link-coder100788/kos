// audio: an audio analyzer, in a window of its own.
//
//     audio <file>
//
// Shows a sound file three ways, one tab at a time: its waveform, a
// spectrogram (time across, frequency up, loudness in magma colors) and its
// frequency spectrum. Reads WAV (8/16/24/32-bit PCM, 32/64-bit float,
// µ-law, A-law and IMA ADPCM), AIFF / AIFF-C and Sun .au / .snd files.
//
// 1, 2, 3 or Tab switch tabs. In the waveform and spectrogram the mouse
// wheel (or + and -) zooms around the pointer, right-drag (or the arrow
// keys) scrolls, and left-drag selects a stretch: the spectrum then shows
// only that, and Space plays only that. A click clears the selection, z
// zooms to it, 0 shows the whole file. [ and ] change the FFT size, l
// switches the frequency axis between linear and log, Space plays / stops.
//
// Typed in the Terminal, it reads the file (printing what's wrong with it,
// if anything), opens the window as its own child and gives the Terminal
// back. Playing is another child, `audio --play`, since play_file waits
// until the sound is over: stopping ends it.

#include "../libc/sys.h"
#include "../libc/stdio.h"
#include "../libc/stdlib.h"
#include "../libc/string.h"
#include "../libc/math.h"
#include "../libc/canvas.h"
#include <stdarg.h>

#define SELF "/bin/audio.elf.b64"
#define PLAY_DIR "/.kos/audio"
#define PLAY_FILE "/.kos/audio/play.wav"
// What play_file takes.
#define PLAY_MAX_S 180
#define PLAY_MIN_RATE 8000
#define PLAY_MAX_RATE 192000
#define PLAY_OUT_RATE 48000

// ---- look --------------------------------------------------------------------

#define WIN_W 960
#define WIN_H 620
#define TABS_H 24
#define INFO_H 18
#define FOOTER_H 20
#define PX 60                                   // the plot area
#define PY (TABS_H + INFO_H + 10)
#define PW (WIN_W - PX - 70)
#define PH (WIN_H - PY - FOOTER_H - 36)
#define SCROLL_Y (PY + PH + 4)                  // where the view sits in the file
#define AXIS_Y (SCROLL_Y + 10)                  // time labels

#define BG       0x16161c
#define PANEL    0x0e0e12
#define TABBAR   0x0a0a0d
#define TAB_ON   0x2a2a36
#define GRID     0x2a2a34
#define AXIS     0x55556a
#define TEXT     0xd8d8e4
#define DIM      0x80808f
#define WAVE     0x3c78d8   // peaks
#define WAVE_RMS 0x86b4ff   // RMS, inside the peaks
#define SPEC     0x5aa0ff
#define SPEC_FILL 0x1b3358
#define PLAYHEAD 0x3ddc84
#define CURSOR   0xb0b0c0
#define PEAK     0xffb347
#define ERR      0xff5d7a

// Loudness ranges, in dB below full scale.
#define SPECTRO_FLOOR -100.0f
#define SPECTRUM_FLOOR -120.0f

#define MIN_FFT 256
#define MAX_FFT 16384
#define SUMMARY 256         // frames per waveform summary block
#define WELCH_MAX 400       // most FFTs averaged into a spectrum
#define WORK_MS 40          // analysis time per frame, so the window stays live

// ---- the sound ---------------------------------------------------------------

static int channels;
static uint32_t rate;
static long frames;
static int16_t** pcm;           // pcm[channel][frame]
static char kind[48];           // "WAV, 16-bit PCM"
static char name[128];
static float peak_db, rms_db;

static long sum_blocks;
static int16_t** sum_min;       // per channel, per SUMMARY frames
static int16_t** sum_max;
static float** sum_sq;

// ---- decoding ----------------------------------------------------------------
// Every format comes down to interleaved samples of one encoding, except
// IMA ADPCM, which has its own decoder.

enum { E_U8, E_S8, E_S16, E_S24, E_S32, E_F32, E_F64, E_ULAW, E_ALAW };
static const int enc_width[] = { 1, 1, 2, 3, 4, 4, 8, 1, 1 };

static char error[160];

static int fail(const char* format, ...) __attribute__((format(printf, 1, 2)));
static int fail(const char* format, ...) {
    va_list args;
    va_start(args, format);
    vsnprintf(error, sizeof error, format, args);
    va_end(args);
    return -1;
}

static uint32_t le16(const uint8_t* p) { return p[0] | p[1] << 8; }
static uint32_t le32(const uint8_t* p) { return p[0] | p[1] << 8 | p[2] << 16 | (uint32_t)p[3] << 24; }
static uint32_t be16(const uint8_t* p) { return p[0] << 8 | p[1]; }
static uint32_t be32(const uint8_t* p) { return (uint32_t)p[0] << 24 | p[1] << 16 | p[2] << 8 | p[3]; }

static int16_t ulaw(uint8_t u) {
    u = ~u;
    int t = (((u & 0x0f) << 3) + 0x84) << ((u & 0x70) >> 4);
    return (u & 0x80) ? 0x84 - t : t - 0x84;
}

static int16_t alaw(uint8_t a) {
    a ^= 0x55;
    int t = (a & 0x0f) << 4;
    int seg = (a & 0x70) >> 4;
    if (seg == 0) t += 8;
    else if (seg == 1) t += 0x108;
    else t = (t + 0x108) << (seg - 1);
    return (a & 0x80) ? t : -t;
}

static int16_t clamp16(double v) {
    v *= 32768.0;
    if (v >= 32767.0) return 32767;
    if (v <= -32768.0) return -32768;
    return (int16_t)(v < 0 ? v - 0.5 : v + 0.5);
}

// One sample at p as 16-bit. big: big-endian.
static int16_t sample(const uint8_t* p, int enc, int big) {
    uint8_t b[8];
    int w = enc_width[enc];
    // Put it in little-endian order.
    for (int i = 0; i < w; i++) b[i] = big ? p[w - 1 - i] : p[i];
    switch (enc) {
    case E_U8:   return (int16_t)((b[0] - 128) << 8);
    case E_S8:   return (int16_t)((int8_t)b[0] << 8);
    case E_S16:  return (int16_t)le16(b);
    case E_S24:  return (int16_t)(le32((uint8_t[]){ 0, b[0], b[1], b[2] }) >> 16);
    case E_S32:  return (int16_t)(le32(b) >> 16);
    case E_F32: { union { uint32_t u; float f; } v = { le32(b) }; return clamp16(v.f); }
    case E_F64: {
        union { uint64_t u; double d; } v = { le32(b) | (uint64_t)le32(b + 4) << 32 };
        return clamp16(v.d);
    }
    case E_ULAW: return ulaw(b[0]);
    default:     return alaw(b[0]);
    }
}

static int alloc_pcm(int ch, long n) {
    if (ch < 1 || ch > 16) return fail("%d channels isn't supported", ch);
    if (n <= 0) return fail("there's no sound in it");
    channels = ch;
    frames = n;
    pcm = alloc_z(sizeof *pcm * ch);
    for (int c = 0; c < ch; c++) {
        pcm[c] = alloc((size_t)n * sizeof(int16_t));
        if (!pcm[c]) return fail("not enough memory for %ld frames", n);
    }
    return 0;
}

static int decode_samples(const uint8_t* data, size_t len, int ch, int enc, int big) {
    size_t frame = (size_t)enc_width[enc] * (ch > 0 ? ch : 1);
    if (alloc_pcm(ch, (long)(len / frame)) < 0) return -1;
    for (long i = 0; i < frames; i++)
        for (int c = 0; c < ch; c++)
            pcm[c][i] = sample(data + i * frame + c * enc_width[enc], enc, big);
    return 0;
}

static const int ima_steps[89] = {
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45,
    50, 55, 60, 66, 73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230,
    253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963,
    1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327,
    3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493, 10442,
    11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794,
    32767,
};
static const int ima_index[16] = { -1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8 };

// WAV's IMA ADPCM: blocks of `align` bytes, each starting with every
// channel's predictor and step index, then 4 bytes (8 samples) of each
// channel in turn.
static int decode_ima(const uint8_t* data, size_t len, int ch, size_t align) {
    if (ch < 1 || align <= (size_t)4 * ch || (align - 4 * ch) % (4 * ch))
        return fail("bad IMA ADPCM block size %zu", align);
    long per_block = 1 + (long)(align - 4 * ch) * 2 / ch;
    long blocks = (long)(len / align);
    if (alloc_pcm(ch, blocks * per_block) < 0) return -1;
    for (long b = 0; b < blocks; b++) {
        const uint8_t* p = data + b * align;
        long base = b * per_block;
        for (int c = 0; c < ch; c++) {
            int pred = (int16_t)le16(p + 4 * c);
            int index = p[4 * c + 2];
            if (index > 88) index = 88;
            pcm[c][base] = pred;
            const uint8_t* q = p + 4 * ch + 4 * c;
            for (long i = 1; i < per_block; q += 4 * ch) {
                for (int k = 0; k < 8 && i < per_block; k++, i++) {
                    int nib = (q[k / 2] >> (k & 1 ? 4 : 0)) & 15;
                    int step = ima_steps[index];
                    int diff = step >> 3;
                    if (nib & 1) diff += step >> 2;
                    if (nib & 2) diff += step >> 1;
                    if (nib & 4) diff += step;
                    pred += nib & 8 ? -diff : diff;
                    if (pred > 32767) pred = 32767;
                    if (pred < -32768) pred = -32768;
                    index += ima_index[nib];
                    if (index < 0) index = 0;
                    if (index > 88) index = 88;
                    pcm[c][base + i] = pred;
                }
            }
        }
    }
    return 0;
}

static int decode_wav(const uint8_t* d, size_t len) {
    int have_fmt = 0, code = 0, ch = 0, bits = 0;
    size_t align = 0;
    size_t at = 12;
    while (at + 8 <= len) {
        const uint8_t* id = d + at;
        size_t n = le32(d + at + 4);
        size_t body = at + 8;
        // Recorders that stop early leave the data chunk's size 0 or too big.
        if (!memcmp(id, "data", 4) && (n == 0 || body + n > len)) n = len - body;
        if (body + n > len) return fail("the '%.4s' chunk is cut short", (const char*)id);
        if (!memcmp(id, "fmt ", 4)) {
            if (n < 16) return fail("the format chunk is too short");
            code = le16(d + body);
            ch = le16(d + body + 2);
            rate = le32(d + body + 4);
            align = le16(d + body + 12);
            bits = le16(d + body + 14);
            if (code == 0xfffe && n >= 26) code = le16(d + body + 24);
            have_fmt = 1;
        } else if (!memcmp(id, "data", 4)) {
            if (!have_fmt) return fail("the data comes before the format");
            const uint8_t* data = d + body;
            int enc = -1;
            switch (code) {
            case 1:
                enc = bits == 8 ? E_U8 : bits == 16 ? E_S16 : bits == 24 ? E_S24 : bits == 32 ? E_S32 : -1;
                snprintf(kind, sizeof kind, "WAV, %d-bit PCM", bits);
                break;
            case 3:
                enc = bits == 32 ? E_F32 : bits == 64 ? E_F64 : -1;
                snprintf(kind, sizeof kind, "WAV, %d-bit float", bits);
                break;
            case 6: enc = E_ALAW; strcpy(kind, "WAV, A-law"); break;
            case 7: enc = E_ULAW; strcpy(kind, "WAV, mu-law"); break;
            case 0x11:
                strcpy(kind, "WAV, IMA ADPCM");
                return decode_ima(data, n, ch, align);
            default:
                return fail("WAV format %#x isn't supported (PCM, float, mu-law, A-law and IMA ADPCM are)", code);
            }
            if (enc < 0) return fail("%s samples aren't supported", kind);
            return decode_samples(data, n, ch, enc, 0);
        }
        at = body + n + (n & 1);
    }
    return fail("no sound data in the file");
}

// AIFF's sample rate is an 80-bit extended float.
static double extended(const uint8_t* p) {
    int exp = (int)(be16(p) & 0x7fff);
    uint64_t mant = (uint64_t)be32(p + 2) << 32 | be32(p + 6);
    if (!exp && !mant) return 0;
    double v = (double)mant;
    for (int e = exp - 16383 - 63; e > 0; e--) v *= 2;
    for (int e = exp - 16383 - 63; e < 0; e++) v /= 2;
    return be16(p) & 0x8000 ? -v : v;
}

static int decode_aiff(const uint8_t* d, size_t len) {
    int aifc = !memcmp(d + 8, "AIFC", 4);
    int have_comm = 0, ch = 0, bits = 0, enc = -1, big = 1;
    long nframes = 0;
    size_t at = 12;
    while (at + 8 <= len) {
        const uint8_t* id = d + at;
        size_t n = be32(d + at + 4);
        size_t body = at + 8;
        if (body + n > len) n = len - body;
        if (!memcmp(id, "COMM", 4)) {
            if (n < 18) return fail("the COMM chunk is too short");
            ch = (int)be16(d + body);
            nframes = be32(d + body + 2);
            bits = (int)be16(d + body + 6);
            rate = (uint32_t)(extended(d + body + 8) + 0.5);
            char comp[5] = "NONE";
            if (aifc && n >= 22) memcpy(comp, d + body + 18, 4);
            int width = (bits + 7) / 8;
            int pcm_enc = width == 1 ? E_S8 : width == 2 ? E_S16 : width == 3 ? E_S24 : width == 4 ? E_S32 : -1;
            if (!strcmp(comp, "NONE") || !strcmp(comp, "twos")) enc = pcm_enc;
            else if (!strcmp(comp, "sowt")) { enc = pcm_enc; big = 0; }
            else if (!strcasecmp(comp, "fl32")) enc = E_F32;
            else if (!strcasecmp(comp, "fl64")) enc = E_F64;
            else if (!strcasecmp(comp, "ulaw")) enc = E_ULAW;
            else if (!strcasecmp(comp, "alaw")) enc = E_ALAW;
            else return fail("AIFF-C compression '%s' isn't supported", comp);
            if (enc < 0) return fail("%d-bit AIFF samples aren't supported", bits);
            if (enc == E_F32 || enc == E_F64) snprintf(kind, sizeof kind, "%s, %d-bit float", aifc ? "AIFF-C" : "AIFF", enc == E_F32 ? 32 : 64);
            else if (enc == E_ULAW || enc == E_ALAW) snprintf(kind, sizeof kind, "AIFF-C, %s", enc == E_ULAW ? "mu-law" : "A-law");
            else snprintf(kind, sizeof kind, "%s, %d-bit PCM%s", aifc ? "AIFF-C" : "AIFF", bits, big ? "" : " (LE)");
            have_comm = 1;
        } else if (!memcmp(id, "SSND", 4)) {
            if (!have_comm) return fail("the sound comes before the COMM chunk");
            if (n < 8) return fail("the SSND chunk is too short");
            size_t offset = be32(d + body);
            if (offset > n - 8) return fail("bad SSND offset");
            size_t bytes = n - 8 - offset;
            size_t want = (size_t)nframes * ch * enc_width[enc];
            if (want < bytes) bytes = want;
            return decode_samples(d + body + 8 + offset, bytes, ch, enc, big);
        }
        at = body + n + (n & 1);
    }
    return fail("no sound data in the file");
}

static int decode_au(const uint8_t* d, size_t len) {
    if (len < 24) return fail("the header is cut short");
    size_t offset = be32(d + 4);
    size_t n = be32(d + 8);
    uint32_t encoding = be32(d + 12);
    rate = be32(d + 16);
    int ch = (int)be32(d + 20);
    if (offset > len) return fail("bad data offset");
    if (n == 0xffffffff || offset + n > len) n = len - offset;
    static const struct { uint32_t code; int enc; const char* what; } kinds[] = {
        { 1, E_ULAW, "mu-law" }, { 2, E_S8, "8-bit PCM" }, { 3, E_S16, "16-bit PCM" },
        { 4, E_S24, "24-bit PCM" }, { 5, E_S32, "32-bit PCM" }, { 6, E_F32, "32-bit float" },
        { 7, E_F64, "64-bit float" }, { 27, E_ALAW, "A-law" },
    };
    for (size_t i = 0; i < sizeof kinds / sizeof *kinds; i++) {
        if (kinds[i].code != encoding) continue;
        snprintf(kind, sizeof kind, "AU, %s", kinds[i].what);
        return decode_samples(d + offset, n, ch, kinds[i].enc, 1);
    }
    return fail("AU encoding %u isn't supported", encoding);
}

static int decode(const uint8_t* d, size_t len) {
    int r;
    if (len >= 12 && !memcmp(d, "RIFF", 4) && !memcmp(d + 8, "WAVE", 4)) r = decode_wav(d, len);
    else if (len >= 12 && !memcmp(d, "FORM", 4) && (!memcmp(d + 8, "AIFF", 4) || !memcmp(d + 8, "AIFC", 4))) r = decode_aiff(d, len);
    else if (len >= 4 && !memcmp(d, ".snd", 4)) r = decode_au(d, len);
    else if (len >= 4 && (!memcmp(d, "fLaC", 4) || !memcmp(d, "OggS", 4) || !memcmp(d, "ID3", 3) || (d[0] == 0xff && (d[1] & 0xe0) == 0xe0)))
        return fail("compressed audio (FLAC, Ogg, MP3) isn't supported yet: WAV, AIFF and AU are");
    else return fail("not a sound file this can read (WAV, AIFF or AU)");
    if (r < 0) return r;
    if (rate < 1 || rate > 1000000) return fail("a sample rate of %u Hz isn't supported", rate);
    return 0;
}

// Reads and decodes `path`, and works out its summary and levels.
static int load(const char* path) {
    size_t len;
    uint8_t* bytes = (uint8_t*)fs_read_all(path, &len);
    if (!bytes) return fail("can't read %s", last_error());
    int r = decode(bytes, len);
    free(bytes);
    if (r < 0) return r;

    sum_blocks = (frames + SUMMARY - 1) / SUMMARY;
    sum_min = alloc(sizeof *sum_min * channels);
    sum_max = alloc(sizeof *sum_max * channels);
    sum_sq = alloc(sizeof *sum_sq * channels);
    double total_sq = 0;
    int peak = 0;
    for (int c = 0; c < channels; c++) {
        sum_min[c] = alloc(sum_blocks * sizeof(int16_t));
        sum_max[c] = alloc(sum_blocks * sizeof(int16_t));
        sum_sq[c] = alloc(sum_blocks * sizeof(float));
        for (long b = 0; b < sum_blocks; b++) {
            int lo = 32767, hi = -32768;
            double sq = 0;
            long end = (b + 1) * SUMMARY < frames ? (b + 1) * SUMMARY : frames;
            for (long i = b * SUMMARY; i < end; i++) {
                int v = pcm[c][i];
                if (v < lo) lo = v;
                if (v > hi) hi = v;
                sq += (double)v * v;
            }
            sum_min[c][b] = lo;
            sum_max[c][b] = hi;
            sum_sq[c][b] = (float)sq;
            total_sq += sq;
            if (-lo > peak) peak = -lo;
            if (hi > peak) peak = hi;
        }
    }
    peak_db = peak ? 20 * log10(peak / 32768.0) : -INFINITY;
    double rms = sqrt(total_sq / ((double)frames * channels)) / 32768.0;
    rms_db = rms > 0 ? 20 * log10(rms) : -INFINITY;
    return 0;
}

// ---- maths -------------------------------------------------------------------

// log2 good to about 0.01, for turning thousands of magnitudes into colors.
static inline float fast_log2(float x) {
    union { float f; uint32_t u; } v = { x };
    float e = (float)(int)((v.u >> 23) & 0xff) - 128.0f;
    v.u = (v.u & 0x007fffff) | 0x3f800000;     // the mantissa, in [1, 2)
    float m = v.f;
    return e + (-0.34484843f * m + 2.02466578f) * m - 0.67487759f;
}

// 10 * log10(power).
static inline float power_db(float p) {
    return p > 1e-30f ? 3.01029996f * fast_log2(p) : -300.0f;
}

static int fft_n;               // the size the tables are for
static float* fft_cos;
static float* fft_sin;
static float* window;           // Hann
static float window_sum;
static float* fre;
static float* fim;

static void fft_setup(int n) {
    if (n == fft_n) return;
    free(fft_cos); free(fft_sin); free(window); free(fre); free(fim);
    fft_n = n;
    fft_cos = alloc(sizeof(float) * n / 2);
    fft_sin = alloc(sizeof(float) * n / 2);
    window = alloc(sizeof(float) * n);
    fre = alloc(sizeof(float) * n);
    fim = alloc(sizeof(float) * n);
    for (int i = 0; i < n / 2; i++) {
        fft_cos[i] = (float)cos(2 * M_PI * i / n);
        fft_sin[i] = (float)-sin(2 * M_PI * i / n);
    }
    window_sum = 0;
    for (int i = 0; i < n; i++) {
        window[i] = (float)(0.5 - 0.5 * cos(2 * M_PI * i / n));
        window_sum += window[i];
    }
}

// The channels' average at frame i, -1..1; silence outside the file.
static inline float mono(long i) {
    if (i < 0 || i >= frames) return 0;
    int s = 0;
    for (int c = 0; c < channels; c++) s += pcm[c][i];
    return s * (1.0f / 32768.0f) / channels;
}

// Power of each bin 0..n/2 of the windowed frames from `start` into out,
// scaled so a full-scale sine is 1 (0 dB).
static void power_spectrum(long start, float* out) {
    int n = fft_n;
    for (int i = 0; i < n; i++) {
        fre[i] = mono(start + i) * window[i];
        fim[i] = 0;
    }
    for (int i = 1, j = 0; i < n; i++) {
        int bit = n >> 1;
        for (; j & bit; bit >>= 1) j ^= bit;
        j |= bit;
        if (i < j) {
            float t = fre[i]; fre[i] = fre[j]; fre[j] = t;
        }
    }
    for (int len = 2; len <= n; len <<= 1) {
        int step = n / len;
        for (int i = 0; i < n; i += len) {
            for (int k = 0; k < len / 2; k++) {
                float wr = fft_cos[k * step], wi = fft_sin[k * step];
                float* ar = &fre[i + k]; float* ai = &fim[i + k];
                float* br = &fre[i + k + len / 2]; float* bi = &fim[i + k + len / 2];
                float tr = *br * wr - *bi * wi;
                float ti = *br * wi + *bi * wr;
                *br = *ar - tr; *bi = *ai - ti;
                *ar += tr; *ai += ti;
            }
        }
    }
    float scale = 2.0f / window_sum;
    scale *= scale;
    for (int i = 0; i <= n / 2; i++) out[i] = (fre[i] * fre[i] + fim[i] * fim[i]) * scale;
}

// ---- view state --------------------------------------------------------------

enum { TAB_WAVE, TAB_SPECTRO, TAB_SPECTRUM };
static const char* tab_names[] = { "1 Waveform", "2 Spectrogram", "3 Spectrum" };
static int tab;

static canvas_t win;
static uint32_t* base;          // everything but the cursor, playhead and footer
static int base_dirty = 1;

static double v0, v1;           // the frames in view
static long sel0 = -1, sel1 = -1;
static int log_freq[3] = { 0, 0, 1 };
static int spectro_n = 1024, spectrum_n = 4096;

static int mouse_x = -1, mouse_y = -1;
static int dragging;            // CANVAS_LEFT or CANVAS_RIGHT
static int drag_x;
static double drag_v0;
static long drag_frame;
static int drag_moved;

static char status[200];
static uint32_t status_color = DIM;

static void set_status(uint32_t color, const char* format, ...) __attribute__((format(printf, 2, 3)));
static void set_status(uint32_t color, const char* format, ...) {
    va_list args;
    va_start(args, format);
    vsnprintf(status, sizeof status, format, args);
    va_end(args);
    status_color = color;
}

static double min_view(void) {
    double m = PW / 16.0;
    return m < frames ? m : frames;
}

static void set_view(double a, double b) {
    double w = b - a;
    if (w < min_view()) {
        double mid = (a + b) / 2;
        w = min_view();
        a = mid - w / 2;
    }
    if (w > frames) w = frames;
    if (a < 0) a = 0;
    if (a + w > frames) a = frames - w;
    if (a != v0 || a + w != v1) base_dirty = 1;
    v0 = a;
    v1 = a + w;
}

static void zoom(double factor, double around) {
    set_view(around - (around - v0) * factor, around + (v1 - around) * factor);
}

static double x_to_frame(int x) { return v0 + (x - PX) * (v1 - v0) / PW; }
static int frame_to_x(double f) { return PX + (int)floor((f - v0) * PW / (v1 - v0)); }

static int in_plot(int x, int y) { return x >= PX && x < PX + PW && y >= PY && y < PY + PH; }

static int has_selection(void) { return sel0 >= 0 && sel1 > sel0; }

// ---- drawing helpers ---------------------------------------------------------

static uint32_t mix(uint32_t a, uint32_t b, int t) {    // t of 256 of b
    int r = ((a >> 16 & 255) * (256 - t) + (b >> 16 & 255) * t) >> 8;
    int g = ((a >> 8 & 255) * (256 - t) + (b >> 8 & 255) * t) >> 8;
    int bl = ((a & 255) * (256 - t) + (b & 255) * t) >> 8;
    return (uint32_t)(r << 16 | g << 8 | bl);
}

static void vline(uint32_t* px, int x, int y0, int y1, uint32_t color) {
    if (x < 0 || x >= WIN_W) return;
    if (y0 > y1) { int t = y0; y0 = y1; y1 = t; }
    if (y0 < 0) y0 = 0;
    if (y1 >= WIN_H) y1 = WIN_H - 1;
    for (int y = y0; y <= y1; y++) px[y * WIN_W + x] = color;
}

static void hline(uint32_t* px, int x0, int x1, int y, uint32_t color) {
    if (y < 0 || y >= WIN_H) return;
    if (x0 < 0) x0 = 0;
    if (x1 >= WIN_W) x1 = WIN_W - 1;
    for (int x = x0; x <= x1; x++) px[y * WIN_W + x] = color;
}

static void text_right(int x, int y, const char* s, uint32_t color) {
    canvas_text(&win, x - canvas_text_width(s, 1), y, s, color, 1);
}

static void text_center(int x, int y, const char* s, uint32_t color) {
    canvas_text(&win, x - canvas_text_width(s, 1) / 2, y, s, color, 1);
}

// A 1, 2 or 5 times a power of ten, about range / ticks.
static double nice_step(double range, int ticks) {
    double raw = range / ticks;
    double mag = pow(10, floor(log10(raw)));
    double n = raw / mag;
    return (n < 1.5 ? 1 : n < 3 ? 2 : n < 7 ? 5 : 10) * mag;
}

static void format_time(char* out, size_t size, double seconds, double step) {
    int decimals = step >= 1 ? 0 : (int)ceil(-log10(step) - 1e-9);
    if (decimals > 6) decimals = 6;
    if (seconds >= 60 && step >= 1) snprintf(out, size, "%d:%02d", (int)seconds / 60, (int)seconds % 60);
    else snprintf(out, size, "%.*fs", decimals, seconds);
}

static void format_hz(char* out, size_t size, double hz) {
    if (hz >= 1000) {
        double k = hz / 1000;
        if (k == floor(k)) snprintf(out, size, "%dk", (int)k);
        else snprintf(out, size, "%.1fk", k);
    } else snprintf(out, size, "%d", (int)(hz + 0.5));
}

static void note_name(char* out, size_t size, double hz) {
    static const char* names[] = { "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B" };
    if (hz < 8 || hz > 30000) { out[0] = 0; return; }
    double midi = 69 + 12 * log(hz / 440.0) / log(2.0);
    int m = (int)floor(midi + 0.5);
    int cents = (int)floor((midi - m) * 100 + 0.5);
    snprintf(out, size, "%s%d %+dc", names[((m % 12) + 12) % 12], m / 12 - 1, cents);
}

static void format_db(char* out, size_t size, double db) {
    if (db < -200 || isnan(db)) snprintf(out, size, "-inf dB");
    else snprintf(out, size, "%.1f dB", db);
}

// ---- magma -------------------------------------------------------------------
// matplotlib's magma, at 0, 0.1, ... 1, spread over 256 colors.

static uint32_t magma[256];

static void magma_setup(void) {
    static const uint32_t stops[11] = {
        0x000004, 0x140e36, 0x3b0f70, 0x641a80, 0x8c2981, 0xb73779,
        0xde4968, 0xf7705c, 0xfe9f6d, 0xfecf92, 0xfcfdbf,
    };
    for (int i = 0; i < 256; i++) {
        int seg = i * 10 / 255;
        if (seg > 9) seg = 9;
        int t = (i * 10 - seg * 255) * 256 / 255;
        magma[i] = mix(stops[seg], stops[seg + 1], t);
    }
}

static uint32_t db_color(float db, float floor_db) {
    int i = (int)((db - floor_db) * 255.0f / -floor_db);
    if (i < 0) i = 0;
    if (i > 255) i = 255;
    return magma[i];
}

// ---- frequency axes ----------------------------------------------------------

#define LOG_LO 20.0

static double nyquist(void) { return rate / 2.0; }

// Where frequency `hz` is, from 0 (bottom / left) to 1.
static double freq_pos(double hz, int is_log) {
    if (!is_log) return hz / nyquist();
    if (hz < LOG_LO) hz = LOG_LO;
    return log(hz / LOG_LO) / log(nyquist() / LOG_LO);
}

static double pos_freq(double pos, int is_log) {
    if (!is_log) return pos * nyquist();
    return LOG_LO * exp(pos * log(nyquist() / LOG_LO));
}

// Calls tick(hz) for each labeled frequency.
static void freq_ticks(int is_log, void (*tick)(double hz)) {
    if (is_log) {
        static const double marks[] = { 20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000, 50000, 100000 };
        for (size_t i = 0; i < sizeof marks / sizeof *marks; i++)
            if (marks[i] <= nyquist()) tick(marks[i]);
    } else {
        double step = nice_step(nyquist(), 8);
        for (double f = 0; f <= nyquist() + 1e-6; f += step) tick(f);
    }
}

// ---- time axis ---------------------------------------------------------------

static void draw_time_axis(void) {
    double secs = (v1 - v0) / rate;
    double step = nice_step(secs, 9);
    double t = ceil(v0 / rate / step) * step;
    for (; t * rate <= v1; t += step) {
        int x = frame_to_x(t * rate);
        vline(base, x, PY, PY + PH - 1, GRID);
        vline(base, x, PY + PH, PY + PH + 2, AXIS);
        char label[24];
        format_time(label, sizeof label, t, step);
        text_center(x, AXIS_Y, label, DIM);
    }
    // Where the view is in the file.
    canvas_rect(&win, PX, SCROLL_Y, PW, 4, GRID);
    int a = PX + (int)(v0 * PW / frames), b = PX + (int)(v1 * PW / frames);
    canvas_rect(&win, a, SCROLL_Y, b - a > 2 ? b - a : 2, 4, AXIS);
}

// ---- the waveform ------------------------------------------------------------

static int lane_top(int c) { return PY + c * PH / channels; }
static int lane_h(int c) { return lane_top(c + 1) - lane_top(c); }

static int amp_y(int c, double v) {      // v in -1..1
    int h = lane_h(c);
    return lane_top(c) + (int)((1 - v) * (h - 1) / 2);
}

static void draw_waveform(void) {
    double spp = (v1 - v0) / PW;    // frames per pixel
    for (int c = 0; c < channels; c++) {
        int top = lane_top(c), h = lane_h(c);
        if (c > 0) hline(base, PX, PX + PW - 1, top, AXIS);
        static const double grid[] = { 1, 0.5, 0, -0.5, -1 };
        for (int g = 0; g < 5; g++) {
            int y = amp_y(c, grid[g]);
            if (grid[g] == 0) hline(base, PX, PX + PW - 1, y, AXIS);
            else if (h > 60) hline(base, PX, PX + PW - 1, y, GRID);
            // Lanes' 1.0 and -1.0 would run into each other.
            if ((h > 60 && (channels == 1 || (g != 0 && g != 4))) || grid[g] == 0) {
                char label[8];
                snprintf(label, sizeof label, "%.1f", grid[g]);
                text_right(PX - 6, y - 3 + (g == 0 ? 4 : g == 4 ? -4 : 0), label, DIM);
            }
        }
        if (channels > 1) {
            char label[12];
            if (channels == 2) strcpy(label, c ? "R" : "L");
            else snprintf(label, sizeof label, "%d", c + 1);
            canvas_text(&win, PX + PW + 8, top + 4, label, DIM, 1);
        }

        if (spp >= 1) {
            for (int x = 0; x < PW; x++) {
                long a = (long)(v0 + x * spp), b = (long)(v0 + (x + 1) * spp);
                if (b <= a) b = a + 1;
                if (b > frames) b = frames;
                int lo = 32767, hi = -32768;
                double sq = 0;
                long i = a;
                // Whole summary blocks where there are some, frames at the ends.
                while (i < b) {
                    if (i % SUMMARY == 0 && i + SUMMARY <= b) {
                        long k = i / SUMMARY;
                        if (sum_min[c][k] < lo) lo = sum_min[c][k];
                        if (sum_max[c][k] > hi) hi = sum_max[c][k];
                        sq += sum_sq[c][k];
                        i += SUMMARY;
                    } else {
                        int v = pcm[c][i++];
                        if (v < lo) lo = v;
                        if (v > hi) hi = v;
                        sq += (double)v * v;
                    }
                }
                if (lo > hi) continue;
                double rms = sqrt(sq / (b - a)) / 32768.0;
                double vlo = lo / 32768.0, vhi = hi / 32768.0;
                vline(base, PX + x, amp_y(c, vhi), amp_y(c, vlo), WAVE);
                double rlo = rms < -vlo ? -rms : vlo, rhi = rms < vhi ? rms : vhi;
                if (rhi > rlo) vline(base, PX + x, amp_y(c, rhi), amp_y(c, rlo), WAVE_RMS);
            }
        } else {
            // Zoomed in past one frame a pixel: join the samples, and mark
            // them once they're far enough apart.
            long a = (long)floor(v0), b = (long)ceil(v1);
            if (b >= frames) b = frames - 1;
            int px = -1, py = 0;
            for (long i = a; i <= b; i++) {
                int x = PX + (int)((i + 0.5 - v0) / spp);
                int y = amp_y(c, pcm[c][i] / 32768.0);
                if (px >= 0) canvas_line(&win, px, py, x, y, WAVE_RMS);
                if (1 / spp >= 6) canvas_rect(&win, x - 1, y - 1, 3, 3, WAVE_RMS);
                px = x; py = y;
            }
        }
    }
}

// ---- the spectrogram ---------------------------------------------------------
// Worked out a few columns at a time (see work()), for the current view,
// FFT size and axis: spectro_key says which that was.

static float* spectro_db;       // PW x PH, column by column
static uint32_t* spectro_img;
static float* bins;             // a power spectrum
static int spectro_done = PW;
static double spectro_key[4] = { -1 };

static void spectro_check(void) {
    double key[4] = { v0, v1, spectro_n, log_freq[TAB_SPECTRO] };
    if (!memcmp(key, spectro_key, sizeof key)) return;
    memcpy(spectro_key, key, sizeof key);
    spectro_done = 0;
    for (int i = 0; i < PW * PH; i++) spectro_img[i] = PANEL;
}

// The bins row y covers: [*lo, *hi].
static void row_bins(int y, int n, int is_log, int* lo, int* hi) {
    double top = pos_freq((double)(PH - y) / PH, is_log);
    double bottom = pos_freq((double)(PH - y - 1) / PH, is_log);
    double per_bin = (double)rate / n;
    *lo = (int)ceil(bottom / per_bin);
    *hi = (int)floor(top / per_bin);
    if (*hi < *lo) *lo = *hi = (int)floor((bottom + top) / 2 / per_bin + 0.5);
    if (*hi > n / 2) *hi = n / 2;
    if (*lo > *hi) *lo = *hi;
}

static void spectro_column(int x) {
    fft_setup(spectro_n);
    double center = v0 + (x + 0.5) * (v1 - v0) / PW;
    power_spectrum((long)center - spectro_n / 2, bins);
    for (int y = 0; y < PH; y++) {
        int lo, hi;
        row_bins(y, spectro_n, log_freq[TAB_SPECTRO], &lo, &hi);
        float p = 0;
        for (int k = lo; k <= hi; k++) if (bins[k] > p) p = bins[k];
        float db = power_db(p);
        spectro_db[x * PH + y] = db;
        spectro_img[y * PW + x] = db_color(db, SPECTRO_FLOOR);
    }
}

static int tick_log;           // which axis freq_ticks' callbacks draw

static void spectro_tick(double hz) {
    int y = PY + PH - 1 - (int)(freq_pos(hz, tick_log) * (PH - 1));
    hline(base, PX - 3, PX - 1, y, AXIS);
    char label[16];
    format_hz(label, sizeof label, hz);
    text_right(PX - 6, y - 3, label, DIM);
}

static void draw_spectrogram(void) {
    for (int y = 0; y < PH; y++)
        memcpy(base + (PY + y) * WIN_W + PX, spectro_img + y * PW, PW * sizeof(uint32_t));
    tick_log = log_freq[TAB_SPECTRO];
    freq_ticks(tick_log, spectro_tick);
    text_right(PX - 6, PY - 10, "Hz", DIM);

    // The color bar.
    int bx = PX + PW + 12;
    for (int y = 0; y < PH; y++)
        hline(base, bx, bx + 10, PY + y, magma[255 - y * 255 / (PH - 1)]);
    for (int db = 0; db >= (int)SPECTRO_FLOOR; db -= 20) {
        int y = PY + (int)((double)db / SPECTRO_FLOOR * (PH - 1));
        char label[8];
        snprintf(label, sizeof label, "%d", db);
        canvas_text(&win, bx + 14, y - 3, label, DIM, 1);
    }
    canvas_text(&win, bx + 14, PY - 10, "dB", DIM, 1);
    if (spectro_done < PW) {
        char label[40];
        snprintf(label, sizeof label, "analyzing... %d%%", spectro_done * 100 / PW);
        text_right(PX + PW, PY - 10, label, DIM);
    }
}

// ---- the spectrum ------------------------------------------------------------
// The average (Welch) power spectrum of the selection, or the whole file.

static float* spectrum_sum;     // summed powers, bins 0..spectrum_n/2
static int spectrum_segments, spectrum_done;
static long spectrum_from, spectrum_hop;
static double spectrum_key[3] = { -1 };

static void spectrum_check(void) {
    long a = has_selection() ? sel0 : 0, b = has_selection() ? sel1 : frames;
    double key[3] = { a, b, spectrum_n };
    if (!memcmp(key, spectrum_key, sizeof key)) return;
    memcpy(spectrum_key, key, sizeof key);
    free(spectrum_sum);
    spectrum_sum = alloc_z(sizeof(float) * (spectrum_n / 2 + 1));
    // Half-overlapping windows, or WELCH_MAX of them spread evenly.
    long len = b - a;
    long hop = spectrum_n / 2;
    long count = len > spectrum_n ? (len - spectrum_n) / hop + 1 : 1;
    if (count > WELCH_MAX) {
        hop = (len - spectrum_n) / (WELCH_MAX - 1);
        count = WELCH_MAX;
    }
    spectrum_from = len > spectrum_n ? a : a + len / 2 - spectrum_n / 2;
    spectrum_hop = hop;
    spectrum_segments = (int)count;
    spectrum_done = 0;
}

static void spectrum_step(void) {
    fft_setup(spectrum_n);
    power_spectrum(spectrum_from + spectrum_done * spectrum_hop, bins);
    for (int k = 0; k <= spectrum_n / 2; k++) spectrum_sum[k] += bins[k];
    spectrum_done++;
}

static float spectrum_db_at(int k) {
    if (!spectrum_done) return -300;
    return power_db(spectrum_sum[k] / spectrum_done);
}

static float spectrum_curve[PW];    // the dB drawn at each column

static int spectrum_y(float db) {
    if (db > 0) db = 0;
    if (db < SPECTRUM_FLOOR) db = SPECTRUM_FLOOR;
    return PY + (int)(db / SPECTRUM_FLOOR * (PH - 1));
}

static double spectrum_x_hz(int x) { return pos_freq((x + 0.5) / PW, log_freq[TAB_SPECTRUM]); }

static void spectrum_tick(double hz) {
    int x = PX + (int)(freq_pos(hz, tick_log) * (PW - 1));
    vline(base, x, PY, PY + PH - 1, GRID);
    vline(base, x, PY + PH, PY + PH + 2, AXIS);
    char label[16];
    format_hz(label, sizeof label, hz);
    text_center(x, PY + PH + 6, label, DIM);
}

// The loudest bin, refined between its neighbors: *hz and *db.
static void spectrum_peak(double* hz, float* db) {
    int best = 1;
    for (int k = 1; k <= spectrum_n / 2; k++)
        if (spectrum_sum[k] > spectrum_sum[best]) best = k;
    double shift = 0;
    if (best > 0 && best < spectrum_n / 2) {
        double a = spectrum_db_at(best - 1), b = spectrum_db_at(best), c = spectrum_db_at(best + 1);
        double d = a - 2 * b + c;
        if (d < 0) shift = 0.5 * (a - c) / d;
    }
    *hz = (best + shift) * rate / spectrum_n;
    *db = spectrum_db_at(best);
}

static void draw_spectrum(void) {
    for (int db = 0; db >= (int)SPECTRUM_FLOOR; db -= 10) {
        int y = spectrum_y((float)db);
        if (db % 20 == 0) {
            hline(base, PX, PX + PW - 1, y, GRID);
            char label[8];
            snprintf(label, sizeof label, "%d", db);
            text_right(PX - 6, y - 3, label, DIM);
        }
    }
    text_right(PX - 6, PY - 12, "dB", DIM);
    tick_log = log_freq[TAB_SPECTRUM];
    freq_ticks(tick_log, spectrum_tick);
    text_right(PX + PW, PY + PH + 18, "Hz", DIM);

    if (!spectrum_done) return;
    double per_bin = (double)rate / spectrum_n;
    for (int x = 0; x < PW; x++) {
        double f0 = pos_freq((double)x / PW, tick_log), f1 = pos_freq((double)(x + 1) / PW, tick_log);
        int lo = (int)ceil(f0 / per_bin), hi = (int)floor(f1 / per_bin);
        float db;
        if (hi >= lo) {
            db = -300;
            for (int k = lo; k <= hi && k <= spectrum_n / 2; k++) {
                float d = spectrum_db_at(k);
                if (d > db) db = d;
            }
        } else {
            // Between two bins: interpolate.
            double at = (f0 + f1) / 2 / per_bin;
            int k = (int)floor(at);
            if (k >= spectrum_n / 2) k = spectrum_n / 2 - 1;
            double t = at - k;
            db = (float)(spectrum_db_at(k) * (1 - t) + spectrum_db_at(k + 1) * t);
        }
        spectrum_curve[x] = db;
    }
    for (int x = 0; x < PW; x++) vline(base, PX + x, spectrum_y(spectrum_curve[x]), PY + PH - 1, SPEC_FILL);
    for (int x = 1; x < PW; x++)
        canvas_line(&win, PX + x - 1, spectrum_y(spectrum_curve[x - 1]), PX + x, spectrum_y(spectrum_curve[x]), SPEC);

    double hz;
    float db;
    spectrum_peak(&hz, &db);
    if (db > SPECTRUM_FLOOR) {
        int x = PX + (int)(freq_pos(hz, tick_log) * (PW - 1));
        canvas_circle(&win, x, spectrum_y(db), 3, PEAK);
        char note[24], label[80];
        note_name(note, sizeof note, hz);
        snprintf(label, sizeof label, "peak %.1f Hz %s  %.1f dB", hz, note, db);
        canvas_text(&win, PX + 8, PY + 6, label, PEAK, 1);
    }
    char what[120];
    long a = has_selection() ? sel0 : 0, b = has_selection() ? sel1 : frames;
    snprintf(what, sizeof what, "%s %.3f-%.3fs, FFT %d, %d window%s%s", has_selection() ? "selection" : "whole file",
             (double)a / rate, (double)b / rate, spectrum_n, spectrum_done, spectrum_done == 1 ? "" : "s",
             spectrum_done < spectrum_segments ? "..." : "");
    text_right(PX + PW - 8, PY + 6, what, DIM);
}

// ---- the window --------------------------------------------------------------

static void draw_base(void) {
    uint32_t* screen = win.pixels;
    win.pixels = base;      // so canvas_* draw into the base
    canvas_clear(&win, BG);

    // Tabs, and what the file is.
    canvas_rect(&win, 0, 0, WIN_W, TABS_H, TABBAR);
    int x = 8;
    for (int t = 0; t < 3; t++) {
        int w = canvas_text_width(tab_names[t], 1) + 20;
        if (t == tab) canvas_rect(&win, x, 3, w, TABS_H - 3, TAB_ON);
        canvas_text(&win, x + 10, 9, tab_names[t], t == tab ? TEXT : DIM, 1);
        x += w + 4;
    }
    char shown[48];
    strlcpy(shown, name, sizeof shown);
    text_right(WIN_W - 10, 9, shown, TEXT);
    char info[200], len[24], peak[16], rms[16];
    double secs = (double)frames / rate;
    snprintf(len, sizeof len, "%d:%06.3f", (int)(secs / 60), fmod(secs, 60));
    format_db(peak, sizeof peak, peak_db);
    format_db(rms, sizeof rms, rms_db);
    snprintf(info, sizeof info, "%s   %u Hz   %s   %s (%ld frames)   peak %s   RMS %s", kind, rate,
             channels == 1 ? "mono" : channels == 2 ? "stereo" : "multichannel", len, frames, peak, rms);
    canvas_text(&win, 10, TABS_H + 6, info, DIM, 1);

    canvas_rect(&win, PX, PY, PW, PH, PANEL);
    if (tab == TAB_WAVE) draw_waveform();
    else if (tab == TAB_SPECTRO) draw_spectrogram();
    else draw_spectrum();
    if (tab != TAB_SPECTRUM) draw_time_axis();
    canvas_rect_outline(&win, PX - 1, PY - 1, PW + 2, PH + 2, AXIS);

    win.pixels = screen;
    base_dirty = 0;
}

// ---- playing -----------------------------------------------------------------

enum { STOPPED, STARTING, PLAYING };
static int play_state;
static long player = -1;
static long play_from, play_to;
static uint64_t play_t0;

static void play_stop(void) {
    if (player >= 0 && proc_running(player)) proc_kill(player);
    if (player >= 0) fs_remove(PLAY_FILE);
    player = -1;
    play_state = STOPPED;
}

// Writes frames [a, b) as a mono 16-bit WAV for play_file, at no more
// than PLAY_OUT_RATE (the sound device's rate anyway) to keep it small.
static int write_wav(long a, long b) {
    int k = (int)((rate + PLAY_OUT_RATE - 1) / PLAY_OUT_RATE);
    uint32_t out_rate = rate / k;
    long n = (b - a) / k;
    size_t size = 44 + (size_t)n * 2;
    uint8_t* w = alloc(size);
    if (!w) return fail("not enough memory");
    memcpy(w, "RIFF", 4);
    uint32_t v32 = (uint32_t)(size - 8); memcpy(w + 4, &v32, 4);
    memcpy(w + 8, "WAVEfmt ", 8);
    v32 = 16; memcpy(w + 16, &v32, 4);
    uint16_t v16 = 1; memcpy(w + 20, &v16, 2);         // PCM
    memcpy(w + 22, &v16, 2);                           // one channel
    memcpy(w + 24, &out_rate, 4);
    v32 = out_rate * 2; memcpy(w + 28, &v32, 4);
    v16 = 2; memcpy(w + 32, &v16, 2);
    v16 = 16; memcpy(w + 34, &v16, 2);
    memcpy(w + 36, "data", 4);
    v32 = (uint32_t)(n * 2); memcpy(w + 40, &v32, 4);
    int16_t* out = (int16_t*)(w + 44);
    for (long i = 0; i < n; i++) {
        float sum = 0;
        for (int j = 0; j < k; j++) sum += mono(a + i * k + j);
        out[i] = clamp16(sum / k);
    }
    fs_mkdir("/.kos");
    fs_mkdir(PLAY_DIR);
    long r = fs_write(PLAY_FILE, w, size);
    free(w);
    return r < 0 ? fail("can't write %s: %s", PLAY_FILE, last_error()) : 0;
}

static void play_start(void) {
    if (rate < PLAY_MIN_RATE || rate > PLAY_MAX_RATE) {
        set_status(ERR, "can't play %u Hz sound (%d-%d Hz)", rate, PLAY_MIN_RATE, PLAY_MAX_RATE);
        return;
    }
    play_from = has_selection() ? sel0 : (long)v0;
    play_to = has_selection() ? sel1 : frames;
    int cut = 0;
    if (play_to - play_from > (long)PLAY_MAX_S * rate) {
        play_to = play_from + (long)PLAY_MAX_S * rate;
        cut = 1;
    }
    if (write_wav(play_from, play_to) < 0) {
        set_status(ERR, "%s", error);
        return;
    }
    player = spawn(SELF, "--play " PLAY_FILE);
    if (player < 0) {
        set_status(ERR, "couldn't start playing: %s", last_error());
        return;
    }
    play_state = STARTING;
    set_status(DIM, cut ? "playing the first 3 minutes" : "");
}

static void play_poll(void) {
    if (play_state == STOPPED) return;
    int running = proc_running(player);
    if (play_state == STARTING) {
        if (snd_busy()) {
            play_state = PLAYING;
            play_t0 = uptime_ms();
        } else if (!running) {
            set_status(ERR, "couldn't play it (is there a sound device?)");
            play_stop();
        }
    } else if (!running || !snd_busy()) {
        play_stop();
    }
}

static double playhead(void) {
    return play_from + (double)(uptime_ms() - play_t0) * rate / 1000.0;
}

// ---- the footer and what's on top --------------------------------------------

static void readout(char* out, size_t size) {
    out[0] = 0;
    if (!in_plot(mouse_x, mouse_y)) return;
    if (tab == TAB_SPECTRUM) {
        if (!spectrum_done) return;
        double hz = spectrum_x_hz(mouse_x - PX);
        char note[24], db[16];
        note_name(note, sizeof note, hz);
        format_db(db, sizeof db, spectrum_curve[mouse_x - PX]);
        snprintf(out, size, "%.1f Hz  %s  %s", hz, note, db);
        return;
    }
    long f = (long)x_to_frame(mouse_x);
    if (f < 0 || f >= frames) return;
    double t = (double)f / rate;
    if (tab == TAB_WAVE) {
        int c = 0;
        while (c + 1 < channels && mouse_y >= lane_top(c + 1)) c++;
        double v = pcm[c][f] / 32768.0;
        char db[16];
        format_db(db, sizeof db, v ? 20 * log10(fabs(v)) : -INFINITY);
        char ch[16] = "";
        if (channels == 2) strcpy(ch, c ? "R  " : "L  ");
        else if (channels > 2) snprintf(ch, sizeof ch, "ch %d  ", c + 1);
        snprintf(out, size, "%.4fs  frame %ld  %s%+.4f  %s", t, f, ch, v, db);
    } else {
        double hz = pos_freq((PH - (mouse_y - PY) - 0.5) / PH, log_freq[TAB_SPECTRO]);
        char note[24], db[16] = "";
        note_name(note, sizeof note, hz);
        int col = mouse_x - PX;
        if (col < spectro_done) format_db(db, sizeof db, spectro_db[col * PH + mouse_y - PY]);
        snprintf(out, size, "%.4fs  %.1f Hz  %s  %s", t, hz, note, db);
    }
}

static void draw_overlay(void) {
    uint32_t* px = win.pixels;
    memcpy(px, base, sizeof(uint32_t) * WIN_W * WIN_H);

    if (tab != TAB_SPECTRUM) {
        if (has_selection()) {
            int a = frame_to_x(sel0), b = frame_to_x(sel1);
            if (a < PX) a = PX;
            if (b > PX + PW) b = PX + PW;
            for (int y = PY; y < PY + PH; y++)
                for (int x = a; x < b; x++) px[y * WIN_W + x] = mix(px[y * WIN_W + x], 0xffffff, 48);
        }
        if (play_state == PLAYING) {
            int x = frame_to_x(playhead());
            if (x >= PX && x < PX + PW) vline(px, x, PY, PY + PH - 1, PLAYHEAD);
        }
        if (in_plot(mouse_x, mouse_y)) vline(px, mouse_x, PY, PY + PH - 1, CURSOR);
    } else if (in_plot(mouse_x, mouse_y) && spectrum_done) {
        vline(px, mouse_x, PY, PY + PH - 1, CURSOR);
        canvas_circle(&win, mouse_x, spectrum_y(spectrum_curve[mouse_x - PX]), 3, TEXT);
    }
    if (tab == TAB_SPECTRO && in_plot(mouse_x, mouse_y)) hline(px, PX, PX + PW - 1, mouse_y, CURSOR);

    int fy = WIN_H - FOOTER_H;
    canvas_rect(&win, 0, fy, WIN_W, FOOTER_H, TABBAR);
    char line[200];
    readout(line, sizeof line);
    if (line[0]) canvas_text(&win, 10, fy + 6, line, TEXT, 1);
    else if (status[0]) canvas_text(&win, 10, fy + 6, status, status_color, 1);
    else canvas_text(&win, 10, fy + 6, "wheel zoom  right-drag scroll  drag select  space play  [ ] FFT  l axis", DIM, 1);
    char right[120] = "";
    if (has_selection())
        snprintf(right, sizeof right, "sel %.3f-%.3fs (%.3fs)", (double)sel0 / rate, (double)sel1 / rate, (double)(sel1 - sel0) / rate);
    else if (tab != TAB_SPECTRUM)
        snprintf(right, sizeof right, "view %.3f-%.3fs", v0 / rate, v1 / rate);
    if (play_state != STOPPED) {
        size_t len = strlen(right);
        snprintf(right + len, sizeof right - len, "%s", play_state == PLAYING ? "  playing" : "  starting...");
    }
    text_right(WIN_W - 10, fy + 6, right, play_state == PLAYING ? PLAYHEAD : DIM);
}

// ---- input -------------------------------------------------------------------

static void set_tab(int t) {
    if (t != tab) base_dirty = 1;
    tab = t;
}

static void fft_size(int bigger) {
    int* n = tab == TAB_SPECTRUM ? &spectrum_n : &spectro_n;
    int m = bigger ? *n * 2 : *n / 2;
    if (m < MIN_FFT || m > MAX_FFT) return;
    *n = m;
    set_status(DIM, "FFT size %d (%.1f Hz a bin, %.1f ms)", m, (double)rate / m, 1000.0 * m / rate);
    base_dirty = 1;
}

static void key(int k) {
    double mid = (v0 + v1) / 2, w = v1 - v0;
    switch (k) {
    case '1': set_tab(TAB_WAVE); break;
    case '2': set_tab(TAB_SPECTRO); break;
    case '3': set_tab(TAB_SPECTRUM); break;
    case '\t': set_tab((tab + 1) % 3); break;
    case ' ':
        if (play_state != STOPPED) play_stop();
        else play_start();
        break;
    case '+': case '=': case CANVAS_KEY_UP: zoom(0.5, mid); break;
    case '-': case '_': case CANVAS_KEY_DOWN: zoom(2, mid); break;
    case CANVAS_KEY_LEFT: set_view(v0 - w / 8, v1 - w / 8); break;
    case CANVAS_KEY_RIGHT: set_view(v0 + w / 8, v1 + w / 8); break;
    case CANVAS_KEY_PAGE_UP: set_view(v0 - w, v1 - w); break;
    case CANVAS_KEY_PAGE_DOWN: set_view(v0 + w, v1 + w); break;
    case CANVAS_KEY_HOME: set_view(0, w); break;
    case CANVAS_KEY_END: set_view(frames - w, frames); break;
    case '0': set_view(0, frames); break;
    case 'z': if (has_selection()) set_view(sel0, sel1); break;
    case 27: sel0 = sel1 = -1; base_dirty = 1; break;
    case 'l': case 'L':
        if (tab == TAB_WAVE) break;
        log_freq[tab] = !log_freq[tab];
        base_dirty = 1;
        break;
    case '[': fft_size(0); break;
    case ']': fft_size(1); break;
    }
}

static void mouse(canvas_event_t* e) {
    int timeline = tab != TAB_SPECTRUM;
    // Scrolls don't say where the pointer is: it's where it last moved.
    if (e->kind == CANVAS_SCROLL) {
        if (timeline && in_plot(mouse_x, mouse_y)) zoom(pow(0.8, e->key), x_to_frame(mouse_x));
        return;
    }
    mouse_x = e->x;
    mouse_y = e->y;
    if (e->kind == CANVAS_MOUSE_DOWN) {
        if (e->y < TABS_H) {
            int x = 8;
            for (int t = 0; t < 3; t++) {
                int w = canvas_text_width(tab_names[t], 1) + 20;
                if (e->x >= x && e->x < x + w) set_tab(t);
                x += w + 4;
            }
            return;
        }
        if (!timeline || !in_plot(e->x, e->y)) return;
        dragging = e->buttons & (CANVAS_RIGHT | CANVAS_MIDDLE) ? CANVAS_RIGHT : CANVAS_LEFT;
        drag_x = e->x;
        drag_v0 = v0;
        drag_frame = (long)x_to_frame(e->x);
        drag_moved = 0;
    } else if (e->kind == CANVAS_MOUSE_MOVE && dragging) {
        if (abs(e->x - drag_x) > 2) drag_moved = 1;
        if (dragging == CANVAS_RIGHT) {
            double shift = (drag_x - e->x) * (v1 - v0) / PW;
            set_view(drag_v0 + shift, drag_v0 + shift + (v1 - v0));
        } else if (drag_moved) {
            long f = (long)x_to_frame(e->x);
            if (f < 0) f = 0;
            if (f > frames) f = frames;
            sel0 = f < drag_frame ? f : drag_frame;
            sel1 = f < drag_frame ? drag_frame : f;
        }
    } else if (e->kind == CANVAS_MOUSE_UP && dragging) {
        if (dragging == CANVAS_LEFT && !drag_moved) sel0 = sel1 = -1;
        dragging = 0;
    }
}

// Analysis for at most WORK_MS. Returns whether there was any.
static int work(void) {
    uint64_t start = uptime_ms();
    int did = 0;
    if (tab == TAB_SPECTRO) {
        spectro_check();
        while (spectro_done < PW && uptime_ms() - start < WORK_MS) {
            spectro_column(spectro_done++);
            did = 1;
        }
    } else if (tab == TAB_SPECTRUM) {
        spectrum_check();
        while (spectrum_done < spectrum_segments && uptime_ms() - start < WORK_MS) {
            spectrum_step();
            did = 1;
        }
    }
    return did;
}

__attribute__((noreturn)) static void window_main(const char* path) {
    if (load(path) < 0) {
        printf("audio: %s\n", error);
        exit(0);
    }
    const char* slash = strrchr(path, '/');
    strlcpy(name, slash ? slash + 1 : path, sizeof name);
    char title[160];
    snprintf(title, sizeof title, "Audio - %s", name);
    if (canvas_open(&win, title, WIN_W, WIN_H) < 0) {
        printf("audio: couldn't open a window\n");
        exit(0);
    }
    base = alloc(sizeof(uint32_t) * WIN_W * WIN_H);
    spectro_db = alloc(sizeof(float) * PW * PH);
    spectro_img = alloc(sizeof(uint32_t) * PW * PH);
    bins = alloc(sizeof(float) * (MAX_FFT / 2 + 1));
    magma_setup();
    set_view(0, frames);

    for (;;) {
        canvas_event_t e;
        int input = 0;
        while (canvas_poll(&win, &e) > 0) {
            input = 1;
            if (e.kind == CANVAS_CLOSED) {
                play_stop();
                canvas_close(&win);
                exit(0);
            }
            if (e.kind == CANVAS_KEY) key(e.key);
            else mouse(&e);
        }
        int was = play_state;
        play_poll();
        if (play_state == PLAYING) {
            // Keep the playhead in view: turn the page when it runs off.
            double p = playhead();
            if (p > v1 && p < play_to) set_view(p, p + (v1 - v0));
        }
        int busy = work();
        if (busy) base_dirty = 1;
        int show = input || busy || base_dirty || play_state != STOPPED || was != play_state;
        if (base_dirty) draw_base();
        if (show) {
            draw_overlay();
            canvas_present(&win);
        }
        if (busy) yield();
        else sleep_ms(15);
    }
}

void _start(void) {
    char raw[512];
    long n = args(raw, sizeof raw - 1);
    if (n > (long)sizeof raw - 1) n = sizeof raw - 1;
    raw[n] = 0;
    char* arg = raw;
    while (*arg == ' ') arg++;
    for (char* end = arg + strlen(arg); end > arg && end[-1] == ' '; ) *--end = 0;

    if (!strncmp(arg, "--play ", 7)) {
        if (play_file(arg + 7) < 0) printf("audio: can't play: %s\n", last_error());
        exit(0);
    }
    if (!strncmp(arg, "--window ", 9)) window_main(arg + 9);

    if (!*arg) {
        printf("usage: audio <file>   (WAV, AIFF or AU)\n");
        exit(0);
    }
    // Started from a shell, which waits for us: check the file, then open
    // the window as our own child, with an absolute path, and end so the
    // Terminal gets its prompt back.
    char path[400];
    if (arg[0] == '/') strlcpy(path, arg, sizeof path);
    else {
        char cwd[256];
        if (fs_cwd(cwd, sizeof cwd) < 0) strcpy(cwd, "/");
        if (!strcmp(cwd, "/")) snprintf(path, sizeof path, "/%s", arg);
        else snprintf(path, sizeof path, "%s/%s", cwd, arg);
    }
    if (load(path) < 0) {
        printf("audio: %s\n", error);
        exit(0);
    }
    char window_args[420];
    snprintf(window_args, sizeof window_args, "--window %s", path);
    if (spawn(SELF, window_args) < 0) printf("audio: couldn't start: %s\n", last_error());
    exit(0);
}
