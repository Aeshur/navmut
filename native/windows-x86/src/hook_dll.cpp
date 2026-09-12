#include "navmut_native/ipc_names.h"
#include "navmut_native/protocol.h"
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

#include <array>
#include <cstdint>
#include <cstring>
#include <span>
#include <string>
#include <string_view>

namespace
{

using namespace navmut::native;

static_assert(sizeof(void*) == sizeof(std::uint32_t), "the hook DLL must be built as a 32-bit target");

struct HookContext
{
    HANDLE mapping = nullptr;
    HANDLE request_event = nullptr;
    HANDLE response_event = nullptr;
    HANDLE stop_event = nullptr;
    SharedBlock* shared = nullptr;
};

HookContext g_context;

void close_context()
{
    if (g_context.shared != nullptr)
    {
        UnmapViewOfFile(g_context.shared);
        g_context.shared = nullptr;
    }
    if (g_context.mapping != nullptr)
    {
        CloseHandle(g_context.mapping);
        g_context.mapping = nullptr;
    }
    if (g_context.request_event != nullptr)
    {
        CloseHandle(g_context.request_event);
        g_context.request_event = nullptr;
    }
    if (g_context.response_event != nullptr)
    {
        CloseHandle(g_context.response_event);
        g_context.response_event = nullptr;
    }
    if (g_context.stop_event != nullptr)
    {
        CloseHandle(g_context.stop_event);
        g_context.stop_event = nullptr;
    }
}

bool open_context(std::uint32_t nonce_low, std::uint32_t nonce_high)
{
    if (nonce_low == 0 && nonce_high == 0)
    {
        return false;
    }
    const std::uint32_t process_id = GetCurrentProcessId();
    if (g_context.shared != nullptr)
    {
        return shared_header_valid(*g_context.shared, process_id, nonce_low, nonce_high);
    }
    g_context.mapping = OpenFileMappingW(FILE_MAP_ALL_ACCESS, FALSE,
                                         ipc_mapping_name(process_id, nonce_low, nonce_high).c_str());
    g_context.request_event = OpenEventW(SYNCHRONIZE, FALSE,
                                         ipc_request_event_name(process_id, nonce_low, nonce_high).c_str());
    g_context.response_event = OpenEventW(SYNCHRONIZE | EVENT_MODIFY_STATE, FALSE,
                                          ipc_response_event_name(process_id, nonce_low, nonce_high).c_str());
    g_context.stop_event = OpenEventW(SYNCHRONIZE, FALSE,
                                      ipc_stop_event_name(process_id, nonce_low, nonce_high).c_str());
    if (g_context.mapping == nullptr || g_context.request_event == nullptr || g_context.response_event == nullptr
        || g_context.stop_event == nullptr)
    {
        close_context();
        return false;
    }
    g_context.shared = reinterpret_cast<SharedBlock*>(MapViewOfFile(g_context.mapping, FILE_MAP_ALL_ACCESS, 0, 0,
                                                                      sizeof(SharedBlock)));
    if (g_context.shared == nullptr
        || !shared_header_valid(*g_context.shared, process_id, nonce_low, nonce_high))
    {
        close_context();
        return false;
    }
    return true;
}

bool valid_local_read(std::uint32_t address, std::size_t size)
{
    if (address < 0x10000U || size == 0 || static_cast<std::uint64_t>(address) + size > 0x80000000ULL)
    {
        return false;
    }
    MEMORY_BASIC_INFORMATION info{};
    if (VirtualQuery(reinterpret_cast<const void*>(static_cast<std::uintptr_t>(address)), &info, sizeof(info)) == 0)
    {
        return false;
    }
    const std::uint32_t base = static_cast<std::uint32_t>(reinterpret_cast<std::uintptr_t>(info.BaseAddress));
    const std::uint64_t end = static_cast<std::uint64_t>(base) + info.RegionSize;
    return info.State == MEM_COMMIT && (info.Protect & PAGE_GUARD) == 0
        && (info.Protect & 0xFFU) != PAGE_NOACCESS && address >= base
        && static_cast<std::uint64_t>(address) + size <= end;
}

bool read_u32(std::uint32_t address, std::uint32_t& value)
{
    if (!valid_local_read(address, sizeof(value)))
    {
        return false;
    }
    __try
    {
        value = *reinterpret_cast<volatile std::uint32_t*>(static_cast<std::uintptr_t>(address));
    }
    __except (EXCEPTION_EXECUTE_HANDLER)
    {
        return false;
    }
    return true;
}

bool read_bytes(std::uint32_t address, std::uint8_t* destination, std::size_t size)
{
    if (!valid_local_read(address, size))
    {
        return false;
    }
    __try
    {
        std::memcpy(destination, reinterpret_cast<const void*>(static_cast<std::uintptr_t>(address)), size);
    }
    __except (EXCEPTION_EXECUTE_HANDLER)
    {
        return false;
    }
    return true;
}

bool safe_add(std::uint32_t left, std::uint32_t right, std::uint32_t& result)
{
    const std::uint64_t sum = static_cast<std::uint64_t>(left) + right;
    if (sum > 0xFFFFFFFFULL)
    {
        return false;
    }
    result = static_cast<std::uint32_t>(sum);
    return true;
}

bool validate_target_chain(const SharedBlock& shared, std::string& reason)
{
    if (GetCurrentProcessId() != shared.target_pid || GetCurrentThreadId() != shared.target_thread)
    {
        reason = "request arrived on the wrong target thread";
        return false;
    }
    std::uint32_t scene_vtable = 0;
    std::uint32_t outer = 0;
    std::uint32_t container = 0;
    std::uint32_t outer_field = 0;
    std::uint32_t actor = 0;
    std::uint32_t actor_vtable = 0;
    if (!read_u32(shared.scene_address, scene_vtable) || scene_vtable != kSceneVtable
        || !safe_add(shared.scene_address, kSceneOuterOffset, outer_field)
        || !read_u32(outer_field, outer)
        || !safe_add(outer, 0x10U, container)
        || !safe_add(container, kContainerActorOffset, actor)
        || !read_u32(actor, actor)
        || !read_u32(actor, actor_vtable))
    {
        reason = "the player scene chain is no longer readable";
        return false;
    }
    const SceneChainSample scene{shared.scene_address, scene_vtable, outer, container, actor, actor_vtable};
    if (!validate_scene_chain(scene, reason) || outer != shared.scene_outer || actor != shared.actor
        || actor_vtable != shared.actor_vtable)
    {
        if (reason.empty())
        {
            reason = "the player scene chain changed";
        }
        return false;
    }

    std::uint32_t chat_vtable = 0;
    std::uint32_t chat_member_address = 0;
    std::uint32_t chat_member = 0;
    if (!read_u32(shared.chat_manager, chat_vtable) || chat_vtable != kChatManagerVtable
        || !safe_add(shared.chat_manager, kChatManagerMemberOffset, chat_member_address)
        || !read_u32(chat_member_address, chat_member))
    {
        reason = "the chat manager is no longer readable";
        return false;
    }
    const ChatManagerSample chat{shared.chat_manager, chat_vtable, chat_member};
    if (!validate_chat_manager(chat, reason))
    {
        return false;
    }
    return true;
}

bool copy_command(const SharedBlock& shared, char (&command)[kMaxCommandBytes], std::string& reason)
{
    if (shared.command_length == 0 || shared.command_length >= sizeof(command)
        || shared.command[shared.command_length] != '\0')
    {
        reason = "the helper command buffer is malformed";
        return false;
    }
    for (std::size_t index = 0; index < shared.command_length; ++index)
    {
        const unsigned char character = static_cast<unsigned char>(shared.command[index]);
        if (character < 0x20U || character > 0x7EU)
        {
            reason = "the helper command is not printable ASCII";
            return false;
        }
    }
    if (shared.command_length < 5 || std::memcmp(shared.command, "!pos ", 5) != 0)
    {
        reason = "the helper command is not a !pos request";
        return false;
    }
    std::memset(command, 0, sizeof(command));
    std::memcpy(command, shared.command, shared.command_length);
    if (!validate_retail_position(std::string_view(command, shared.command_length)))
    {
        reason = "the helper command failed numeric validation";
        return false;
    }
    return true;
}

bool validate_prologues(std::string& reason)
{
    const std::array<std::uint32_t, 4> addresses{
        kUtf8Constructor,
        kUtf8Assign,
        kUtf8Destructor,
        kChatEncoder,
    };
    const std::array<const std::array<std::uint8_t, kPrologueBytes>*, 4> expected{
        &kUtf8ConstructorPrologue,
        &kUtf8AssignPrologue,
        &kUtf8DestructorPrologue,
        &kChatEncoderPrologue,
    };
    for (std::size_t index = 0; index < addresses.size(); ++index)
    {
        std::array<std::uint8_t, kPrologueBytes> actual{};
        if (!read_bytes(addresses[index], actual.data(), actual.size()))
        {
            reason = "the retail function prologue is no longer readable";
            return false;
        }
        if (!compare_exact_prologue(actual, std::span<const std::uint8_t>(*expected[index])))
        {
            reason = "the retail function prologue changed before the call";
            return false;
        }
    }
    return true;
}

#pragma pack(push, 1)
struct RetailUtf8
{
    std::uint8_t storage[0x54]{};
};
#pragma pack(pop)

static_assert(sizeof(RetailUtf8) == 0x54, "the retail UTF-8 value must match the verified ABI");

using Utf8Constructor = void(__thiscall*)(RetailUtf8*);
using Utf8Assign = void(__thiscall*)(RetailUtf8*, const char*);
using Utf8Destructor = void(__thiscall*)(RetailUtf8*);
using ChatEncoder = bool(__thiscall*)(void*, void*, RetailUtf8*, RetailUtf8*, int);

enum class RetailCallStatus
{
    Success,
    Rejected,
    Unknown,
};

// Keep SEH in a small POD function. MSVC rejects __try in a function
// whose stack contains objects requiring C++ unwinding.
RetailCallStatus invoke_retail(std::uint32_t chat_manager, std::uint32_t actor, const char* command)
{
    RetailUtf8 empty_name;
    RetailUtf8 retail_command;
    bool empty_constructed = false;
    bool command_constructed = false;
    bool call_exception = false;
    bool result = false;
    __try
    {
        const auto construct = reinterpret_cast<Utf8Constructor>(kUtf8Constructor);
        const auto assign = reinterpret_cast<Utf8Assign>(kUtf8Assign);
        const auto encode = reinterpret_cast<ChatEncoder>(kChatEncoder);
        construct(&empty_name);
        empty_constructed = true;
        construct(&retail_command);
        command_constructed = true;
        assign(&retail_command, command);
        result = encode(reinterpret_cast<void*>(static_cast<std::uintptr_t>(chat_manager)),
                        reinterpret_cast<void*>(static_cast<std::uintptr_t>(actor)), &empty_name, &retail_command, 1);
    }
    __except (EXCEPTION_EXECUTE_HANDLER)
    {
        call_exception = true;
    }

    bool destructor_exception = false;
    __try
    {
        const auto destruct = reinterpret_cast<Utf8Destructor>(kUtf8Destructor);
        if (command_constructed)
        {
            destruct(&retail_command);
        }
        if (empty_constructed)
        {
            destruct(&empty_name);
        }
    }
    __except (EXCEPTION_EXECUTE_HANDLER)
    {
        destructor_exception = true;
    }
    if (call_exception || destructor_exception)
    {
        return RetailCallStatus::Unknown;
    }
    return result ? RetailCallStatus::Success : RetailCallStatus::Rejected;
}

void complete_request(ResponseCode code, std::string_view message)
{
    if (g_context.shared == nullptr)
    {
        return;
    }
    const std::uint32_t sequence = g_context.shared->request_sequence;
    set_shared_response(*g_context.shared, sequence, code, message);
    MemoryBarrier();
    InterlockedExchange(reinterpret_cast<volatile LONG*>(&g_context.shared->request_state),
                        static_cast<LONG>(RequestState::Complete));
    SetEvent(g_context.response_event);
}

void process_request()
{
    const DWORD stop_wait = g_context.stop_event == nullptr ? WAIT_FAILED : WaitForSingleObject(g_context.stop_event, 0);
    if (g_context.shared == nullptr || stop_wait != WAIT_TIMEOUT)
    {
        return;
    }
    auto* state = reinterpret_cast<volatile LONG*>(&g_context.shared->request_state);
    if (InterlockedCompareExchange(state, static_cast<LONG>(RequestState::Processing),
                                    static_cast<LONG>(RequestState::Pending))
        != static_cast<LONG>(RequestState::Pending))
    {
        return;
    }

    char command[kMaxCommandBytes]{};
    std::string reason;
    if (!copy_command(*g_context.shared, command, reason))
    {
        complete_request(ResponseCode::Error, reason);
        return;
    }
    if (!validate_target_chain(*g_context.shared, reason) || !validate_prologues(reason))
    {
        complete_request(ResponseCode::Error, reason);
        return;
    }

    const RetailCallStatus call = invoke_retail(g_context.shared->chat_manager, g_context.shared->actor, command);
    if (call == RetailCallStatus::Success)
    {
        complete_request(ResponseCode::Ok, "");
    }
    else if (call == RetailCallStatus::Rejected)
    {
        complete_request(ResponseCode::Error, "the retail encoder rejected the command");
    }
    else
    {
        complete_request(ResponseCode::Unknown, "the retail call raised an access violation");
    }
}

} // namespace

extern "C" LRESULT CALLBACK NavmutCallWndProc(int code, WPARAM wparam, LPARAM lparam)
{
    if (code >= 0 && lparam != 0)
    {
        const auto* message = reinterpret_cast<const CWPSTRUCT*>(lparam);
        const auto nonce_low = static_cast<std::uint32_t>(message->wParam);
        const auto nonce_high = static_cast<std::uint32_t>(message->lParam);
        if (message->message == kWakeMessage && open_context(nonce_low, nonce_high)
            && reinterpret_cast<std::uintptr_t>(message->hwnd) == g_context.shared->target_window)
        {
            InterlockedExchange(reinterpret_cast<volatile LONG*>(&g_context.shared->hook_ready), 1);
            if (WaitForSingleObject(g_context.request_event, 0) == WAIT_OBJECT_0)
            {
                try
                {
                    process_request();
                }
                catch (...)
                {
                    complete_request(ResponseCode::Unknown, "the hook failed while preparing the request");
                }
            }
        }
    }
    return CallNextHookEx(nullptr, code, wparam, lparam);
}

BOOL WINAPI DllMain(HINSTANCE instance, DWORD reason, LPVOID reserved)
{
    (void)instance;
    (void)reserved;
    if (reason == DLL_PROCESS_ATTACH)
    {
        DisableThreadLibraryCalls(instance);
    }
    else if (reason == DLL_PROCESS_DETACH)
    {
        close_context();
    }
    return TRUE;
}
