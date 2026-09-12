#pragma once

#include "navmut_native/retail_layout.h"

#include <cstddef>
#include <cstdint>
#include <span>
#include <string>

namespace navmut::native
{

struct SceneChainSample
{
    std::uint32_t scene_address = 0;
    std::uint32_t scene_vtable = 0;
    std::uint32_t outer = 0;
    std::uint32_t container = 0;
    std::uint32_t actor = 0;
    std::uint32_t actor_vtable = 0;
};

struct ChatManagerSample
{
    std::uint32_t object = 0;
    std::uint32_t vtable = 0;
    std::uint32_t member = 0;
};

bool validate_scene_chain(const SceneChainSample& sample, std::string& reason);
bool validate_chat_manager(const ChatManagerSample& sample, std::string& reason);
bool compare_exact_prologue(std::span<const std::uint8_t> actual,
                            std::span<const std::uint8_t> expected);

} // namespace navmut::native
