#include "navmut_native/hook_context.h"

#include "navmut_native/ipc_names.h"
#include "navmut_native/shared_state.h"

namespace navmut::native
{

void close_hook_context(HookContext& context)
{
    if (context.shared != nullptr)
    {
        UnmapViewOfFile(context.shared);
        context.shared = nullptr;
    }
    if (context.mapping != nullptr)
    {
        CloseHandle(context.mapping);
        context.mapping = nullptr;
    }
    if (context.request_event != nullptr)
    {
        CloseHandle(context.request_event);
        context.request_event = nullptr;
    }
    if (context.response_event != nullptr)
    {
        CloseHandle(context.response_event);
        context.response_event = nullptr;
    }
    if (context.stop_event != nullptr)
    {
        CloseHandle(context.stop_event);
        context.stop_event = nullptr;
    }
}

bool open_hook_context(HookContext& context, std::uint32_t process_id, std::uint32_t nonce_low,
                       std::uint32_t nonce_high)
{
    if (nonce_low == 0 && nonce_high == 0)
    {
        return false;
    }
    if (context.shared != nullptr && shared_header_valid(*context.shared, process_id, nonce_low, nonce_high))
    {
        return true;
    }
    HookContext next;
    next.mapping = OpenFileMappingW(FILE_MAP_ALL_ACCESS, FALSE,
                                    ipc_mapping_name(process_id, nonce_low, nonce_high).c_str());
    next.request_event = OpenEventW(SYNCHRONIZE, FALSE,
                                    ipc_request_event_name(process_id, nonce_low, nonce_high).c_str());
    next.response_event = OpenEventW(SYNCHRONIZE | EVENT_MODIFY_STATE, FALSE,
                                     ipc_response_event_name(process_id, nonce_low, nonce_high).c_str());
    next.stop_event = OpenEventW(SYNCHRONIZE, FALSE,
                                 ipc_stop_event_name(process_id, nonce_low, nonce_high).c_str());
    if (next.mapping == nullptr || next.request_event == nullptr || next.response_event == nullptr
        || next.stop_event == nullptr)
    {
        close_hook_context(next);
        return false;
    }
    next.shared = reinterpret_cast<SharedBlock*>(MapViewOfFile(next.mapping, FILE_MAP_ALL_ACCESS, 0, 0,
                                                                 sizeof(SharedBlock)));
    if (next.shared == nullptr || !shared_header_valid(*next.shared, process_id, nonce_low, nonce_high))
    {
        close_hook_context(next);
        return false;
    }
    close_hook_context(context);
    context = next;
    return true;
}

} // namespace navmut::native
