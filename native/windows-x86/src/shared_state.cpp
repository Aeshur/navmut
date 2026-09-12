#include "navmut_native/shared_state.h"

#include <algorithm>
#include <cstring>

namespace navmut::native
{

void initialise_shared_block(SharedBlock& block, std::uint32_t process_id, std::uint32_t nonce_low,
                             std::uint32_t nonce_high, std::uint32_t thread_id, std::uint32_t window)
{
    block = SharedBlock{};
    block.magic = kSharedMagic;
    block.version = kSharedVersion;
    block.size = static_cast<std::uint32_t>(sizeof(SharedBlock));
    block.target_pid = process_id;
    block.nonce_low = nonce_low;
    block.nonce_high = nonce_high;
    block.target_thread = thread_id;
    block.target_window = window;
}

bool shared_header_valid(const SharedBlock& block, std::uint32_t process_id, std::uint32_t nonce_low,
                         std::uint32_t nonce_high)
{
    return block.magic == kSharedMagic && block.version == kSharedVersion
        && block.size == sizeof(SharedBlock) && block.target_pid == process_id && block.nonce_low == nonce_low
        && block.nonce_high == nonce_high
        && block.target_thread != 0 && block.target_window != 0;
}

bool set_shared_response(SharedBlock& block, std::uint32_t sequence, ResponseCode code,
                         std::string_view message)
{
    if (message.size() + 1 > sizeof(block.response))
    {
        return false;
    }
    std::memset(block.response, 0, sizeof(block.response));
    std::memcpy(block.response, message.data(), message.size());
    block.response_length = static_cast<std::uint32_t>(message.size());
    block.response_sequence = sequence;
    block.response_code = static_cast<std::int32_t>(code);
    block.request_state = static_cast<std::int32_t>(RequestState::Complete);
    return true;
}

} // namespace navmut::native
