#pragma once

#include "navmut_native/retail_layout.h"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>

#include <cstdint>

namespace navmut::native
{

struct HookContext
{
    HANDLE mapping = nullptr;
    HANDLE request_event = nullptr;
    HANDLE response_event = nullptr;
    HANDLE stop_event = nullptr;
    SharedBlock* shared = nullptr;
};

void close_hook_context(HookContext& context);

// The hook DLL can outlive its helper session. A new session's wake replaces
// the old session only after the new one opens; a stray wake must not detach
// a live helper.
bool open_hook_context(HookContext& context, std::uint32_t process_id, std::uint32_t nonce_low,
                       std::uint32_t nonce_high);

} // namespace navmut::native
