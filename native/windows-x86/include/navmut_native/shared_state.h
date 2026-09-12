#pragma once

#include "navmut_native/retail_layout.h"

#include <cstdint>
#include <string_view>

namespace navmut::native
{

void initialise_shared_block(SharedBlock& block, std::uint32_t process_id, std::uint32_t nonce_low,
                             std::uint32_t nonce_high, std::uint32_t thread_id, std::uint32_t window);
bool shared_header_valid(const SharedBlock& block, std::uint32_t process_id, std::uint32_t nonce_low,
                         std::uint32_t nonce_high);
bool set_shared_response(SharedBlock& block, std::uint32_t sequence, ResponseCode code,
                         std::string_view message);

} // namespace navmut::native
