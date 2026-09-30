// Functions the windows crates and arboard import from DLLs Windows 7 lacks, defined here
// under their own names and as the `__imp_` pointers an import library would supply, so the
// linker takes these and the executable never names those DLLs.
#include <string.h>
#include <wchar.h>

typedef long HRESULT;
typedef int BOOL;
#define S_OK ((HRESULT)0)
#define S_FALSE ((HRESULT)1)
#define E_INVALIDARG ((HRESULT)0x80070057L)

__declspec(dllimport) void *__stdcall LoadLibraryA(const char *name);
__declspec(dllimport) void *__stdcall GetProcAddress(void *module, const char *name);

// combase.dll (Windows 8): Windows 7's ole32.dll exports the same two.
static void *ole32(const char *name) {
    return GetProcAddress(LoadLibraryA("ole32.dll"), name);
}

HRESULT __stdcall CoCreateFreeThreadedMarshaler(void *outer, void **marshaler) {
    static HRESULT(__stdcall * real)(void *, void **);
    if (!real) real = (HRESULT(__stdcall *)(void *, void **))ole32("CoCreateFreeThreadedMarshaler");
    return real(outer, marshaler);
}

void __stdcall CoTaskMemFree(void *memory) {
    static void(__stdcall * real)(void *);
    if (!real) real = (void(__stdcall *)(void *))ole32("CoTaskMemFree");
    real(memory);
}

// api-ms-win-core-winrt-error-l1-1-0.dll (Windows 8): nothing records an origin.
BOOL __stdcall RoOriginateErrorW(HRESULT error, unsigned length, const wchar_t *message) {
    (void)error, (void)length, (void)message;
    return 0;
}

// api-ms-win-core-path-l1-1-0.dll (Windows 8): `\\?\C:\x` becomes `C:\x` and
// `\\?\UNC\server\x` becomes `\\server\x`; S_FALSE where there was no prefix.
HRESULT __stdcall PathCchStripPrefix(wchar_t *path, size_t length) {
    if (!path) return E_INVALIDARG;
    if (wcsncmp(path, L"\\\\?\\UNC\\", 8) == 0) {
        if (length < 8) return E_INVALIDARG;
        memmove(path + 2, path + 8, (wcslen(path + 8) + 1) * sizeof(wchar_t));
        return S_OK;
    }
    if (wcsncmp(path, L"\\\\?\\", 4) == 0 && path[4] && path[5] == L':') {
        memmove(path, path + 4, (wcslen(path + 4) + 1) * sizeof(wchar_t));
        return S_OK;
    }
    return S_FALSE;
}

void *__imp_CoCreateFreeThreadedMarshaler = CoCreateFreeThreadedMarshaler;
void *__imp_CoTaskMemFree = CoTaskMemFree;
void *__imp_RoOriginateErrorW = RoOriginateErrorW;
void *__imp_PathCchStripPrefix = PathCchStripPrefix;
