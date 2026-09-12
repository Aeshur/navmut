#include "navmut_native/controller.h"

#if !defined(_WIN32)
#error "The Navmut helper controller is Windows-only"
#endif

#include "navmut_native/ipc_names.h"
#include "navmut_native/retail_layout.h"
#include "navmut_native/shared_state.h"
#include "navmut_native/validation.h"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#include <bcrypt.h>
#include <tlhelp32.h>

#include <algorithm>
#include <array>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <cmath>
#include <filesystem>
#include <fstream>
#include <functional>
#include <limits>
#include <memory>
#include <optional>
#include <sstream>
#include <span>
#include <string>
#include <string_view>
#include <vector>

namespace navmut::native
{
namespace
{

constexpr DWORD kProcessReadRights = PROCESS_QUERY_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION
    | PROCESS_VM_READ | SYNCHRONIZE;
constexpr DWORD kCommandTimeoutMs = 5'000;
constexpr std::uint32_t kScanLimit = 0x80000000U;
constexpr std::size_t kScanChunk = 1024 * 1024;

std::string win32_reason(std::string_view action, DWORD error = GetLastError())
{
    return std::string(action) + " (Win32 error " + std::to_string(error) + ")";
}

bool safe_add(std::uint32_t left, std::uint32_t right, std::uint32_t& result)
{
    const std::uint64_t sum = static_cast<std::uint64_t>(left) + right;
    if (sum > std::numeric_limits<std::uint32_t>::max())
    {
        return false;
    }
    result = static_cast<std::uint32_t>(sum);
    return true;
}

bool read_u16(const std::vector<std::uint8_t>& bytes, std::size_t offset, std::uint16_t& value)
{
    if (offset + sizeof(value) > bytes.size())
    {
        return false;
    }
    value = static_cast<std::uint16_t>(bytes[offset])
        | (static_cast<std::uint16_t>(bytes[offset + 1]) << 8);
    return true;
}

bool read_u32(const std::vector<std::uint8_t>& bytes, std::size_t offset, std::uint32_t& value)
{
    if (offset + sizeof(value) > bytes.size())
    {
        return false;
    }
    value = static_cast<std::uint32_t>(bytes[offset])
        | (static_cast<std::uint32_t>(bytes[offset + 1]) << 8)
        | (static_cast<std::uint32_t>(bytes[offset + 2]) << 16)
        | (static_cast<std::uint32_t>(bytes[offset + 3]) << 24);
    return true;
}

bool read_file(const std::wstring& path, std::vector<std::uint8_t>& bytes, std::string& reason)
{
    HANDLE file = CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                              nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE)
    {
        reason = win32_reason("could not open the FFXIV executable");
        return false;
    }
    LARGE_INTEGER size{};
    if (!GetFileSizeEx(file, &size) || size.QuadPart < 0
        || static_cast<unsigned long long>(size.QuadPart) > std::numeric_limits<std::size_t>::max())
    {
        const DWORD error = GetLastError();
        CloseHandle(file);
        reason = win32_reason("could not inspect the FFXIV executable", error);
        return false;
    }
    bytes.resize(static_cast<std::size_t>(size.QuadPart));
    std::size_t offset = 0;
    while (offset < bytes.size())
    {
        const DWORD request = static_cast<DWORD>(std::min<std::size_t>(bytes.size() - offset, 1024 * 1024));
        DWORD received = 0;
        if (!ReadFile(file, bytes.data() + offset, request, &received, nullptr) || received != request)
        {
            const DWORD error = GetLastError();
            CloseHandle(file);
            reason = win32_reason("could not read the FFXIV executable", error);
            return false;
        }
        offset += received;
    }
    CloseHandle(file);
    return true;
}

bool hash_file(const std::wstring& path, std::array<std::uint8_t, 32>& digest, std::string& reason)
{
    HANDLE file = CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                              nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE)
    {
        reason = win32_reason("could not open the FFXIV executable");
        return false;
    }

    BCRYPT_ALG_HANDLE algorithm = nullptr;
    BCRYPT_HASH_HANDLE hash = nullptr;
    DWORD object_size = 0;
    DWORD result_size = 0;
    NTSTATUS status = BCryptOpenAlgorithmProvider(&algorithm, BCRYPT_SHA256_ALGORITHM, nullptr, 0);
    if (status == 0)
    {
        status = BCryptGetProperty(algorithm, BCRYPT_OBJECT_LENGTH, reinterpret_cast<PUCHAR>(&object_size),
                                   sizeof(object_size), &result_size, 0);
    }
    std::vector<std::uint8_t> object;
    if (status == 0)
    {
        object.resize(object_size);
        status = BCryptCreateHash(algorithm, &hash, object.data(), object_size, nullptr, 0, 0);
    }
    std::vector<std::uint8_t> buffer(1024 * 1024);
    while (status == 0)
    {
        DWORD received = 0;
        if (!ReadFile(file, buffer.data(), static_cast<DWORD>(buffer.size()), &received, nullptr))
        {
            reason = win32_reason("could not read the FFXIV executable");
            status = static_cast<NTSTATUS>(0xC0000001L);
            break;
        }
        if (received == 0)
        {
            break;
        }
        status = BCryptHashData(hash, buffer.data(), received, 0);
    }
    if (status == 0)
    {
        status = BCryptFinishHash(hash, digest.data(), static_cast<ULONG>(digest.size()), 0);
    }
    if (hash != nullptr)
    {
        BCryptDestroyHash(hash);
    }
    if (algorithm != nullptr)
    {
        BCryptCloseAlgorithmProvider(algorithm, 0);
    }
    CloseHandle(file);
    if (status != 0)
    {
        if (reason.empty())
        {
            reason = "could not calculate the FFXIV executable SHA256";
        }
        return false;
    }
    return true;
}

bool generate_nonce(std::uint32_t& nonce_low, std::uint32_t& nonce_high, std::string& reason)
{
    std::array<std::uint8_t, 8> bytes{};
    if (BCryptGenRandom(nullptr, bytes.data(), static_cast<ULONG>(bytes.size()), BCRYPT_USE_SYSTEM_PREFERRED_RNG)
        != 0)
    {
        reason = "could not create the Navmut helper IPC nonce";
        return false;
    }
    std::memcpy(&nonce_low, bytes.data(), sizeof(nonce_low));
    std::memcpy(&nonce_high, bytes.data() + sizeof(nonce_low), sizeof(nonce_high));
    if (nonce_low == 0 && nonce_high == 0)
    {
        reason = "could not create a nonzero Navmut helper IPC nonce";
        return false;
    }
    return true;
}

std::string hex_digest(const std::array<std::uint8_t, 32>& digest)
{
    static constexpr char hex[] = "0123456789abcdef";
    std::string result;
    result.reserve(64);
    for (std::uint8_t byte : digest)
    {
        result.push_back(hex[byte >> 4]);
        result.push_back(hex[byte & 0x0F]);
    }
    return result;
}

class PeImage
{
public:
    bool load(const std::wstring& path, std::string& reason)
    {
        if (!read_file(path, bytes_, reason))
        {
            return false;
        }
        std::uint16_t dos_magic = 0;
        std::uint32_t pe_offset = 0;
        if (!read_u16(bytes_, 0, dos_magic) || dos_magic != 0x5A4DU || !read_u32(bytes_, 0x3CU, pe_offset)
            || pe_offset + 24 > bytes_.size())
        {
            reason = "the FFXIV executable has an invalid PE header";
            return false;
        }
        std::uint32_t signature = 0;
        if (!read_u32(bytes_, pe_offset, signature) || signature != 0x00004550U)
        {
            reason = "the FFXIV executable has an invalid PE signature";
            return false;
        }
        const std::size_t file_header = pe_offset + 4;
        std::uint16_t sections = 0;
        std::uint16_t optional_size = 0;
        if (!read_u16(bytes_, file_header + 2, sections) || !read_u16(bytes_, file_header + 16, optional_size))
        {
            reason = "the FFXIV executable has an invalid PE file header";
            return false;
        }
        const std::size_t optional = file_header + 20;
        if (optional + optional_size > bytes_.size())
        {
            reason = "the FFXIV executable has an invalid PE optional header";
            return false;
        }
        std::uint16_t optional_magic = 0;
        if (!read_u16(bytes_, optional, optional_magic) || optional_magic != 0x10BU
            || !read_u32(bytes_, optional + 28, image_base_) || !read_u32(bytes_, optional + 60, headers_size_))
        {
            reason = "the FFXIV executable is not the supported 32-bit image";
            return false;
        }
        const std::size_t section_table = optional + optional_size;
        if (section_table + static_cast<std::size_t>(sections) * 40 > bytes_.size())
        {
            reason = "the FFXIV executable has an invalid PE section table";
            return false;
        }
        sections_.clear();
        for (std::size_t index = 0; index < sections; ++index)
        {
            const std::size_t section = section_table + index * 40;
            Section value;
            if (!read_u32(bytes_, section + 12, value.virtual_address)
                || !read_u32(bytes_, section + 16, value.raw_size)
                || !read_u32(bytes_, section + 20, value.raw_address))
            {
                reason = "the FFXIV executable has an invalid PE section";
                return false;
            }
            if (value.raw_address > bytes_.size() || value.raw_size > bytes_.size() - value.raw_address)
            {
                reason = "the FFXIV executable has an invalid PE section range";
                return false;
            }
            sections_.push_back(value);
        }
        if (image_base_ != kImageBase)
        {
            reason = "the FFXIV executable image base does not match the supported client";
            return false;
        }
        return true;
    }

    [[nodiscard]] std::uint32_t image_base() const
    {
        return image_base_;
    }

    bool bytes_at_rva(std::uint32_t rva, std::span<std::uint8_t> destination) const
    {
        const std::uint64_t end = static_cast<std::uint64_t>(rva) + destination.size();
        if (rva < headers_size_ && end <= bytes_.size())
        {
            std::memcpy(destination.data(), bytes_.data() + rva, destination.size());
            return true;
        }
        for (const Section& section : sections_)
        {
            const std::uint64_t section_end = static_cast<std::uint64_t>(section.virtual_address) + section.raw_size;
            if (rva >= section.virtual_address && end <= section_end)
            {
                const std::uint64_t offset = static_cast<std::uint64_t>(section.raw_address)
                    + rva - section.virtual_address;
                if (offset + destination.size() <= bytes_.size())
                {
                    std::memcpy(destination.data(), bytes_.data() + offset, destination.size());
                    return true;
                }
            }
        }
        return false;
    }

private:
    struct Section
    {
        std::uint32_t virtual_address = 0;
        std::uint32_t raw_size = 0;
        std::uint32_t raw_address = 0;
    };

    std::vector<std::uint8_t> bytes_;
    std::vector<Section> sections_;
    std::uint32_t image_base_ = 0;
    std::uint32_t headers_size_ = 0;
};

class ProcessReader
{
public:
    explicit ProcessReader(HANDLE process) : process_(process)
    {
    }

    bool readable(std::uint32_t address, std::size_t size) const
    {
        if (address < 0x10000U || size == 0 || static_cast<std::uint64_t>(address) + size > kScanLimit)
        {
            return false;
        }
        MEMORY_BASIC_INFORMATION info{};
        if (VirtualQueryEx(process_, reinterpret_cast<const void*>(static_cast<std::uintptr_t>(address)), &info,
                           sizeof(info)) == 0)
        {
            return false;
        }
        const std::uint32_t base = static_cast<std::uint32_t>(reinterpret_cast<std::uintptr_t>(info.BaseAddress));
        const std::uint64_t end = static_cast<std::uint64_t>(base) + info.RegionSize;
        const DWORD protection = info.Protect;
        return info.State == MEM_COMMIT && (protection & PAGE_GUARD) == 0
            && (protection & 0xFFU) != PAGE_NOACCESS
            && address >= base && static_cast<std::uint64_t>(address) + size <= end;
    }

    bool read(std::uint32_t address, void* destination, std::size_t size) const
    {
        SIZE_T received = 0;
        return readable(address, size)
            && ReadProcessMemory(process_, reinterpret_cast<const void*>(static_cast<std::uintptr_t>(address)),
                                 destination, size, &received)
            && received == size;
    }

    bool read_u32(std::uint32_t address, std::uint32_t& value) const
    {
        return read(address, &value, sizeof(value));
    }

    void scan_dword(std::uint32_t needle, const std::function<bool(std::uint32_t)>& visitor) const
    {
        std::uint32_t address = 0x10000U;
        std::vector<std::uint8_t> chunk(kScanChunk);
        while (address < kScanLimit)
        {
            MEMORY_BASIC_INFORMATION info{};
            if (VirtualQueryEx(process_, reinterpret_cast<const void*>(static_cast<std::uintptr_t>(address)), &info,
                               sizeof(info)) == 0)
            {
                address += 0x1000U;
                continue;
            }
            const std::uint32_t base = static_cast<std::uint32_t>(reinterpret_cast<std::uintptr_t>(info.BaseAddress));
            const std::uint64_t region_end64 = static_cast<std::uint64_t>(base) + info.RegionSize;
            const std::uint32_t region_end = static_cast<std::uint32_t>(std::min<std::uint64_t>(region_end64, kScanLimit));
            const DWORD protection = info.Protect;
            if (info.State == MEM_COMMIT && (protection & PAGE_GUARD) == 0
                && (protection & 0xFFU) != PAGE_NOACCESS)
            {
                std::uint32_t cursor = std::max(address, base);
                while (cursor < region_end)
                {
                    const std::size_t available = region_end - cursor;
                    const std::size_t amount = std::min<std::size_t>(available, kScanChunk);
                    const std::size_t aligned = amount - (amount % sizeof(std::uint32_t));
                    if (aligned != 0 && read(cursor, chunk.data(), aligned))
                    {
                        for (std::size_t offset = 0; offset + sizeof(std::uint32_t) <= aligned; offset += 4)
                        {
                            std::uint32_t value = 0;
                            std::memcpy(&value, chunk.data() + offset, sizeof(value));
                            if (value == needle && visitor(cursor + static_cast<std::uint32_t>(offset)))
                            {
                                return;
                            }
                        }
                    }
                    if (available <= amount)
                    {
                        break;
                    }
                    cursor += static_cast<std::uint32_t>(amount);
                }
            }
            if (region_end <= address)
            {
                address += 0x1000U;
            }
            else
            {
                address = region_end;
            }
        }
    }

private:
    HANDLE process_ = nullptr;
};

struct TargetSnapshot
{
    HANDLE process = nullptr;
    HWND window = nullptr;
    DWORD thread = 0;
    std::uint32_t scene_address = 0;
    std::uint32_t scene_outer = 0;
    std::uint32_t actor = 0;
    std::uint32_t actor_vtable = 0;
    std::uint32_t chat_manager = 0;
};

static_assert(sizeof(void*) == sizeof(std::uint32_t), "the helper must be built as a 32-bit target");

bool target_is_32_bit(HANDLE process, std::string& reason)
{
    using IsWow64Process2Function = BOOL(WINAPI*)(HANDLE, USHORT*, USHORT*);
    const HMODULE kernel = GetModuleHandleW(L"kernel32.dll");
    const auto is_wow64_process_2 = reinterpret_cast<IsWow64Process2Function>(
        GetProcAddress(kernel, "IsWow64Process2"));
    if (is_wow64_process_2 != nullptr)
    {
        USHORT process_machine = IMAGE_FILE_MACHINE_UNKNOWN;
        USHORT native_machine = IMAGE_FILE_MACHINE_UNKNOWN;
        if (!is_wow64_process_2(process, &process_machine, &native_machine))
        {
            reason = win32_reason("could not determine the FFXIV process architecture");
            return false;
        }
        if (process_machine != IMAGE_FILE_MACHINE_I386
            && !(process_machine == IMAGE_FILE_MACHINE_UNKNOWN && native_machine == IMAGE_FILE_MACHINE_I386))
        {
            reason = "the supported FFXIV client must be 32-bit";
            return false;
        }
        return true;
    }
    using IsWow64ProcessFunction = BOOL(WINAPI*)(HANDLE, PBOOL);
    const auto is_wow64_process = reinterpret_cast<IsWow64ProcessFunction>(GetProcAddress(kernel, "IsWow64Process"));
    if (is_wow64_process == nullptr)
    {
        reason = "could not determine the FFXIV process architecture";
        return false;
    }
    BOOL wow64 = FALSE;
    if (!is_wow64_process(process, &wow64))
    {
        reason = win32_reason("could not determine the FFXIV process architecture");
        return false;
    }
    SYSTEM_INFO system{};
    GetNativeSystemInfo(&system);
    if (wow64 == FALSE && system.wProcessorArchitecture != PROCESSOR_ARCHITECTURE_INTEL)
    {
        reason = "the supported FFXIV client must be 32-bit";
        return false;
    }
    return true;
}

bool find_module_base(DWORD process_id, const std::wstring& image_path, std::uint32_t& base, std::string& reason)
{
    HANDLE snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, process_id);
    if (snapshot == INVALID_HANDLE_VALUE)
    {
        reason = win32_reason("could not enumerate the FFXIV image");
        return false;
    }
    MODULEENTRY32W module{};
    module.dwSize = sizeof(module);
    bool found = false;
    if (Module32FirstW(snapshot, &module) != FALSE)
    {
        do
        {
            if (_wcsicmp(module.szExePath, image_path.c_str()) == 0)
            {
                base = static_cast<std::uint32_t>(reinterpret_cast<std::uintptr_t>(module.modBaseAddr));
                found = true;
                break;
            }
        } while (Module32NextW(snapshot, &module) != FALSE);
    }
    CloseHandle(snapshot);
    if (!found)
    {
        reason = "could not find the FFXIV image in the target process";
    }
    return found;
}

bool inspect_target(std::uint32_t process_id, std::uint32_t window_handle, TargetSnapshot& snapshot,
                    std::string& reason)
{
    if (window_handle == 0)
    {
        reason = "an exact FFXIV window handle is required";
        return false;
    }
    const HWND target_window = reinterpret_cast<HWND>(static_cast<std::uintptr_t>(window_handle));
    if (!IsWindow(target_window))
    {
        reason = "the selected FFXIV window is no longer valid";
        return false;
    }
    DWORD owner_process = 0;
    const DWORD target_thread = GetWindowThreadProcessId(target_window, &owner_process);
    if (target_thread == 0 || owner_process != process_id)
    {
        reason = "the selected window does not belong to the requested FFXIV process";
        return false;
    }
    snapshot.process = OpenProcess(kProcessReadRights, FALSE, process_id);
    if (snapshot.process == nullptr)
    {
        reason = win32_reason("could not open the FFXIV process");
        return false;
    }
    auto fail = [&]() {
        CloseHandle(snapshot.process);
        snapshot.process = nullptr;
        return false;
    };
    if (!target_is_32_bit(snapshot.process, reason))
    {
        return fail();
    }
    std::vector<wchar_t> path_buffer(32768);
    DWORD path_length = static_cast<DWORD>(path_buffer.size());
    if (!QueryFullProcessImageNameW(snapshot.process, 0, path_buffer.data(), &path_length))
    {
        reason = win32_reason("could not identify the FFXIV process");
        return fail();
    }
    const std::wstring image_path(path_buffer.data(), path_length);
    const std::size_t separator = image_path.find_last_of(L"\\/");
    const std::wstring filename = separator == std::wstring::npos ? image_path : image_path.substr(separator + 1);
    if (_wcsicmp(filename.c_str(), L"ffxivgame.exe") != 0)
    {
        reason = "the selected process is not ffxivgame.exe";
        return fail();
    }
    std::vector<std::uint8_t> image_bytes;
    if (!read_file(image_path, image_bytes, reason))
    {
        return fail();
    }
    if (image_bytes.size() != kSupportedClientSize)
    {
        reason = "the FFXIV executable size does not match retail 1.23b";
        return fail();
    }
    std::array<std::uint8_t, 32> digest{};
    if (!hash_file(image_path, digest, reason) || hex_digest(digest) != kSupportedClientSha256)
    {
        if (reason.empty())
        {
            reason = "the FFXIV executable SHA256 does not match retail 1.23b";
        }
        return fail();
    }
    PeImage image;
    if (!image.load(image_path, reason))
    {
        return fail();
    }
    std::uint32_t module_base = 0;
    if (!find_module_base(process_id, image_path, module_base, reason) || module_base != kImageBase)
    {
        if (reason.empty())
        {
            reason = "the FFXIV image is not loaded at the supported base address";
        }
        return fail();
    }

    ProcessReader reader(snapshot.process);
    auto check_prologue = [&](std::uint32_t address,
                              const std::array<std::uint8_t, kPrologueBytes>& expected) {
        std::array<std::uint8_t, kPrologueBytes> disk{};
        if (address < image.image_base()
            || !image.bytes_at_rva(address - image.image_base(), std::span<std::uint8_t>(disk)))
        {
            reason = "the retail function prologue is outside the executable image";
            return false;
        }
        std::array<std::uint8_t, kPrologueBytes> memory{};
        if (!reader.read(address, memory.data(), memory.size()))
        {
            reason = "could not read the retail function prologue";
            return false;
        }
        if (!compare_exact_prologue(disk, expected)
            || !compare_exact_prologue(memory, expected))
        {
            reason = "the retail function prologue does not match the verified executable";
            return false;
        }
        return true;
    };
    if (!check_prologue(kUtf8Constructor, kUtf8ConstructorPrologue)
        || !check_prologue(kUtf8Assign, kUtf8AssignPrologue)
        || !check_prologue(kUtf8Destructor, kUtf8DestructorPrologue)
        || !check_prologue(kChatEncoder, kChatEncoderPrologue))
    {
        return fail();
    }

    std::size_t scene_matches = 0;
    reader.scan_dword(kSceneVtable, [&](std::uint32_t scene_address) {
        std::uint32_t outer_address = 0;
        std::uint32_t outer_field = 0;
        std::uint32_t container_address = 0;
        std::uint32_t actor_address = 0;
        std::uint32_t actor_vtable = 0;
        if (!safe_add(scene_address, kSceneOuterOffset, outer_field)
            || !reader.read_u32(outer_field, outer_address)
            || !safe_add(outer_address, 0x10U, container_address)
            || !safe_add(container_address, kContainerActorOffset, actor_address)
            || !reader.read_u32(actor_address, actor_address)
            || !reader.read_u32(actor_address, actor_vtable))
        {
            return false;
        }
        const SceneChainSample sample{scene_address, kSceneVtable, outer_address, container_address,
                                      actor_address, actor_vtable};
        std::string chain_reason;
        if (!validate_scene_chain(sample, chain_reason))
        {
            return false;
        }
        snapshot.scene_address = scene_address;
        snapshot.scene_outer = outer_address;
        snapshot.actor = actor_address;
        snapshot.actor_vtable = actor_vtable;
        ++scene_matches;
        return scene_matches > 1;
    });
    if (scene_matches != 1)
    {
        reason = scene_matches == 0 ? "could not find the validated player scene chain"
                                    : "found multiple validated player scene chains";
        return fail();
    }

    std::size_t chat_matches = 0;
    reader.scan_dword(kChatManagerVtable, [&](std::uint32_t object) {
        std::uint32_t member = 0;
        if (!safe_add(object, kChatManagerMemberOffset, member) || !reader.read_u32(member, member))
        {
            return false;
        }
        const ChatManagerSample sample{object, kChatManagerVtable, member};
        std::string chat_reason;
        if (!validate_chat_manager(sample, chat_reason))
        {
            return false;
        }
        snapshot.chat_manager = object;
        ++chat_matches;
        return chat_matches > 1;
    });
    if (chat_matches != 1)
    {
        reason = chat_matches == 0 ? "could not find the validated chat manager"
                                   : "found multiple validated chat managers";
        return fail();
    }

    snapshot.window = target_window;
    snapshot.thread = target_thread;
    return true;
}

void close_handle(HANDLE& handle)
{
    if (handle != nullptr)
    {
        CloseHandle(handle);
        handle = nullptr;
    }
}

} // namespace

struct HelperController::Impl
{
    HANDLE process = nullptr;
    HANDLE mapping = nullptr;
    HANDLE request_event = nullptr;
    HANDLE response_event = nullptr;
    HANDLE stop_event = nullptr;
    SharedBlock* shared = nullptr;
    HMODULE hook_module = nullptr;
    HHOOK hook = nullptr;
    HWND window = nullptr;
    DWORD thread = 0;
    std::uint32_t process_id = 0;
    std::uint32_t nonce_low = 0;
    std::uint32_t nonce_high = 0;
    std::uint32_t sequence = 0;
    bool active = false;
    bool request_in_flight = false;
    bool deferred_unload = false;
};

HelperController::HelperController() : impl_(new Impl)
{
}

HelperController::~HelperController()
{
    stop();
    delete impl_;
    impl_ = nullptr;
}

bool HelperController::start(std::uint32_t process_id, std::uint32_t window_handle, std::string& reason)
{
    if (process_id == 0 || process_id > std::numeric_limits<DWORD>::max())
    {
        reason = "pid must be a positive 32-bit integer";
        return false;
    }
    if (impl_->active || impl_->hook_module != nullptr)
    {
        reason = "helper is already attached";
        return false;
    }
    if (!generate_nonce(impl_->nonce_low, impl_->nonce_high, reason))
    {
        return false;
    }
    TargetSnapshot snapshot;
    if (!inspect_target(process_id, window_handle, snapshot, reason))
    {
        return false;
    }

    const auto cleanup = [&]() {
        if (impl_->shared != nullptr)
        {
            UnmapViewOfFile(impl_->shared);
            impl_->shared = nullptr;
        }
        close_handle(impl_->mapping);
        close_handle(impl_->request_event);
        close_handle(impl_->response_event);
        close_handle(impl_->stop_event);
        close_handle(impl_->process);
    };
    impl_->process = snapshot.process;
    impl_->window = snapshot.window;
    impl_->thread = snapshot.thread;
    impl_->process_id = process_id;

    SetLastError(ERROR_SUCCESS);
    impl_->mapping = CreateFileMappingW(INVALID_HANDLE_VALUE, nullptr, PAGE_READWRITE, 0,
                                        static_cast<DWORD>(sizeof(SharedBlock)),
                                        ipc_mapping_name(process_id, impl_->nonce_low, impl_->nonce_high).c_str());
    const DWORD mapping_error = GetLastError();
    if (impl_->mapping == nullptr || mapping_error == ERROR_ALREADY_EXISTS)
    {
        reason = "the Navmut helper shared mapping is already in use";
        cleanup();
        return false;
    }
    impl_->shared = reinterpret_cast<SharedBlock*>(MapViewOfFile(impl_->mapping, FILE_MAP_ALL_ACCESS, 0, 0,
                                                                   sizeof(SharedBlock)));
    SetLastError(ERROR_SUCCESS);
    impl_->request_event = CreateEventW(nullptr, FALSE, FALSE,
                                        ipc_request_event_name(process_id, impl_->nonce_low, impl_->nonce_high).c_str());
    const DWORD request_error = GetLastError();
    SetLastError(ERROR_SUCCESS);
    impl_->response_event = CreateEventW(nullptr, FALSE, FALSE,
                                         ipc_response_event_name(process_id, impl_->nonce_low, impl_->nonce_high).c_str());
    const DWORD response_error = GetLastError();
    SetLastError(ERROR_SUCCESS);
    impl_->stop_event = CreateEventW(nullptr, TRUE, FALSE,
                                     ipc_stop_event_name(process_id, impl_->nonce_low, impl_->nonce_high).c_str());
    const DWORD stop_error = GetLastError();
    if (impl_->shared == nullptr || impl_->request_event == nullptr || impl_->response_event == nullptr
        || impl_->stop_event == nullptr || request_error == ERROR_ALREADY_EXISTS
        || response_error == ERROR_ALREADY_EXISTS || stop_error == ERROR_ALREADY_EXISTS)
    {
        reason = win32_reason("could not create Navmut helper shared state");
        cleanup();
        return false;
    }
    initialise_shared_block(*impl_->shared, process_id, impl_->nonce_low, impl_->nonce_high, impl_->thread,
                            static_cast<std::uint32_t>(reinterpret_cast<std::uintptr_t>(impl_->window)));
    impl_->shared->scene_address = snapshot.scene_address;
    impl_->shared->scene_outer = snapshot.scene_outer;
    impl_->shared->actor = snapshot.actor;
    impl_->shared->actor_vtable = snapshot.actor_vtable;
    impl_->shared->chat_manager = snapshot.chat_manager;

    wchar_t module_path[32768]{};
    const DWORD module_length = GetModuleFileNameW(nullptr, module_path, static_cast<DWORD>(std::size(module_path)));
    if (module_length == 0 || module_length >= std::size(module_path))
    {
        reason = win32_reason("could not locate the Navmut helper DLL");
        cleanup();
        return false;
    }
    std::wstring hook_path(module_path, module_length);
    const std::size_t slash = hook_path.find_last_of(L"\\/");
    hook_path = (slash == std::wstring::npos ? L"" : hook_path.substr(0, slash + 1)) + L"navmut-helper-hook.dll";
    impl_->hook_module = LoadLibraryW(hook_path.c_str());
    if (impl_->hook_module == nullptr)
    {
        reason = win32_reason("could not load the Navmut helper hook DLL");
        cleanup();
        return false;
    }
    const FARPROC procedure = GetProcAddress(impl_->hook_module, "NavmutCallWndProc");
    if (procedure == nullptr)
    {
        reason = win32_reason("the Navmut helper hook DLL has no hook procedure");
        FreeLibrary(impl_->hook_module);
        impl_->hook_module = nullptr;
        cleanup();
        return false;
    }
    impl_->hook = SetWindowsHookExW(WH_CALLWNDPROC, reinterpret_cast<HOOKPROC>(procedure), impl_->hook_module,
                                   impl_->thread);
    if (impl_->hook == nullptr)
    {
        reason = win32_reason("could not install the Navmut helper window-thread hook");
        FreeLibrary(impl_->hook_module);
        impl_->hook_module = nullptr;
        cleanup();
        return false;
    }
    DWORD_PTR handshake_result = 0;
    if (SendMessageTimeoutW(impl_->window, kWakeMessage, static_cast<WPARAM>(impl_->nonce_low),
                            static_cast<LPARAM>(impl_->nonce_high), SMTO_ABORTIFHUNG | SMTO_BLOCK,
                            kCommandTimeoutMs, &handshake_result)
            == 0
        || InterlockedCompareExchange(reinterpret_cast<volatile LONG*>(&impl_->shared->hook_ready), 1, 1) != 1)
    {
        reason = "the Navmut helper hook did not complete its startup handshake";
        impl_->deferred_unload = true;
        stop();
        return false;
    }
    impl_->active = true;
    return true;
}

SubmitResult HelperController::submit(const Position& position)
{
    try
    {
        if (!impl_->active || impl_->shared == nullptr)
        {
            return {SubmitStatus::Error, "helper is not attached"};
        }
        const DWORD process_wait = WaitForSingleObject(impl_->process, 0);
        if (process_wait == WAIT_OBJECT_0)
        {
            impl_->active = false;
            stop();
            return {SubmitStatus::Unknown, "the FFXIV process exited before the command result"};
        }
        if (process_wait == WAIT_FAILED)
        {
            const std::string reason = win32_reason("could not check whether the FFXIV process is still running");
            impl_->active = false;
            stop();
            return {SubmitStatus::Unknown, reason};
        }
        auto* state = reinterpret_cast<volatile LONG*>(&impl_->shared->request_state);
        if (InterlockedCompareExchange(state, static_cast<LONG>(RequestState::Processing),
                                        static_cast<LONG>(RequestState::Idle))
            != static_cast<LONG>(RequestState::Idle))
        {
            return {SubmitStatus::Unknown, "a previous command is still in progress"};
        }
        ++impl_->sequence;
        if (impl_->sequence == 0)
        {
            ++impl_->sequence;
        }
        std::memset(impl_->shared->command, 0, sizeof(impl_->shared->command));
        std::memcpy(impl_->shared->command, position.retail_command.data(), position.retail_command.size());
        impl_->shared->command_length = static_cast<std::uint32_t>(position.retail_command.size());
        impl_->shared->request_sequence = impl_->sequence;
        impl_->shared->response_sequence = 0;
        impl_->shared->response_code = static_cast<LONG>(ResponseCode::None);
        impl_->shared->response_length = 0;
        std::memset(impl_->shared->response, 0, sizeof(impl_->shared->response));
        MemoryBarrier();
        InterlockedExchange(state, static_cast<LONG>(RequestState::Pending));
        impl_->request_in_flight = true;

        if (!ResetEvent(impl_->response_event))
        {
            InterlockedExchange(state, static_cast<LONG>(RequestState::Abandoned));
            impl_->request_in_flight = false;
            impl_->active = false;
            const std::string reason = win32_reason("could not reset the Navmut helper response event");
            stop();
            return {SubmitStatus::Error, reason};
        }
        if (!SetEvent(impl_->request_event))
        {
            InterlockedExchange(state, static_cast<LONG>(RequestState::Abandoned));
            impl_->request_in_flight = false;
            impl_->active = false;
            const std::string reason = win32_reason("could not signal the Navmut helper request event");
            stop();
            return {SubmitStatus::Error, reason};
        }
        DWORD_PTR message_result = 0;
        if (SendMessageTimeoutW(impl_->window, kWakeMessage, static_cast<WPARAM>(impl_->nonce_low),
                                static_cast<LPARAM>(impl_->nonce_high), SMTO_ABORTIFHUNG | SMTO_BLOCK,
                                kCommandTimeoutMs, &message_result)
            == 0)
        {
            InterlockedExchange(state, static_cast<LONG>(RequestState::Abandoned));
            impl_->active = false;
            impl_->deferred_unload = true;
            const std::string reason = "the FFXIV window thread did not complete the position request";
            stop();
            return {SubmitStatus::Unknown, reason};
        }
        const DWORD wait = WaitForSingleObject(impl_->response_event, 0);
        if (wait != WAIT_OBJECT_0)
        {
            InterlockedExchange(state, static_cast<LONG>(RequestState::Abandoned));
            impl_->active = false;
            impl_->request_in_flight = false;
            const std::string reason = wait == WAIT_FAILED
                ? win32_reason("could not inspect the Navmut helper response event")
                : "the FFXIV window hook completed without a position result";
            stop();
            return {SubmitStatus::Unknown, reason};
        }
        MemoryBarrier();
        const auto response_code = static_cast<ResponseCode>(impl_->shared->response_code);
        if (impl_->shared->response_sequence != impl_->sequence
            || impl_->shared->response_length >= sizeof(impl_->shared->response)
            || (response_code != ResponseCode::Ok && response_code != ResponseCode::Error
                && response_code != ResponseCode::Unknown))
        {
            InterlockedExchange(state, static_cast<LONG>(RequestState::Abandoned));
            impl_->active = false;
            impl_->deferred_unload = true;
            impl_->request_in_flight = false;
            stop();
            return {SubmitStatus::Unknown, "the helper response was malformed"};
        }
        const std::string response(impl_->shared->response, impl_->shared->response_length);
        InterlockedExchange(state, static_cast<LONG>(RequestState::Idle));
        impl_->request_in_flight = false;
        if (response_code == ResponseCode::Ok)
        {
            return {SubmitStatus::Ok, {}};
        }
        if (response_code == ResponseCode::Error)
        {
            return {SubmitStatus::Error, response.empty() ? "the injected request was rejected" : response};
        }
        return {SubmitStatus::Unknown, response.empty() ? "the injected request outcome is unknown" : response};
    }
    catch (...)
    {
        const bool request_in_flight = impl_ != nullptr && impl_->request_in_flight;
        if (impl_ != nullptr)
        {
            impl_->active = false;
            if (request_in_flight)
            {
                impl_->deferred_unload = true;
            }
        }
        stop();
        return {SubmitStatus::Unknown, "the helper failed while preparing the command"};
    }
}

void HelperController::stop() noexcept
{
    if (impl_ == nullptr)
    {
        return;
    }
    if (impl_->request_in_flight)
    {
        impl_->deferred_unload = true;
    }
    if (impl_->stop_event != nullptr)
    {
        SetEvent(impl_->stop_event);
    }
    if (impl_->shared != nullptr)
    {
        auto* state = reinterpret_cast<volatile LONG*>(&impl_->shared->request_state);
        const LONG previous = InterlockedCompareExchange(state, static_cast<LONG>(RequestState::Abandoned),
                                                          static_cast<LONG>(RequestState::Pending));
        if (previous == static_cast<LONG>(RequestState::Pending))
        {
            SetEvent(impl_->request_event);
        }
        else if (previous == static_cast<LONG>(RequestState::Processing))
        {
            InterlockedExchange(state, static_cast<LONG>(RequestState::Abandoned));
        }
    }
    if (impl_->hook != nullptr)
    {
        if (!UnhookWindowsHookEx(impl_->hook))
        {
            impl_->deferred_unload = true;
        }
        impl_->hook = nullptr;
    }
    if (impl_->hook_module != nullptr && !impl_->deferred_unload)
    {
        FreeLibrary(impl_->hook_module);
        impl_->hook_module = nullptr;
    }
    if (impl_->shared != nullptr)
    {
        UnmapViewOfFile(impl_->shared);
        impl_->shared = nullptr;
    }
    close_handle(impl_->mapping);
    close_handle(impl_->request_event);
    close_handle(impl_->response_event);
    close_handle(impl_->stop_event);
    close_handle(impl_->process);
    impl_->active = false;
    impl_->request_in_flight = false;
}

} // namespace navmut::native
