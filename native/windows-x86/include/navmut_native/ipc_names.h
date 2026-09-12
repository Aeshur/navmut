#pragma once

#include <cstdint>
#include <string>

namespace navmut::native
{

inline std::wstring ipc_base_name(std::uint32_t process_id, std::uint32_t nonce_low,
                                  std::uint32_t nonce_high)
{
    return L"Local\\NavmutHelper." + std::to_wstring(process_id) + L"."
        + std::to_wstring(nonce_low) + L"." + std::to_wstring(nonce_high);
}

inline std::wstring ipc_mapping_name(std::uint32_t process_id, std::uint32_t nonce_low,
                                     std::uint32_t nonce_high)
{
    return ipc_base_name(process_id, nonce_low, nonce_high) + L".mapping";
}

inline std::wstring ipc_request_event_name(std::uint32_t process_id, std::uint32_t nonce_low,
                                           std::uint32_t nonce_high)
{
    return ipc_base_name(process_id, nonce_low, nonce_high) + L".request";
}

inline std::wstring ipc_response_event_name(std::uint32_t process_id, std::uint32_t nonce_low,
                                            std::uint32_t nonce_high)
{
    return ipc_base_name(process_id, nonce_low, nonce_high) + L".response";
}

inline std::wstring ipc_stop_event_name(std::uint32_t process_id, std::uint32_t nonce_low,
                                        std::uint32_t nonce_high)
{
    return ipc_base_name(process_id, nonce_low, nonce_high) + L".stop";
}

} // namespace navmut::native
