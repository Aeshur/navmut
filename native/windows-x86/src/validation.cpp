#include "navmut_native/validation.h"

namespace navmut::native
{
namespace
{

bool valid_pointer(std::uint32_t value)
{
    return value >= 0x10000U && value < 0x80000000U;
}

} // namespace

bool validate_scene_chain(const SceneChainSample& sample, std::string& reason)
{
    if (!valid_pointer(sample.scene_address) || sample.scene_address % 4U != 0)
    {
        reason = "scene address is invalid";
        return false;
    }
    if (sample.scene_vtable != kSceneVtable)
    {
        reason = "scene vtable mismatch";
        return false;
    }
    if (!valid_pointer(sample.outer))
    {
        reason = "scene outer is unavailable";
        return false;
    }
    if (static_cast<std::uint64_t>(sample.outer) + 0x10U != sample.container)
    {
        reason = "scene container offset mismatch";
        return false;
    }
    if (!valid_pointer(sample.actor))
    {
        reason = "player actor is unavailable";
        return false;
    }
    if (sample.actor_vtable != kPlayerActorVtable)
    {
        reason = "player actor vtable mismatch";
        return false;
    }
    return true;
}

bool validate_chat_manager(const ChatManagerSample& sample, std::string& reason)
{
    if (!valid_pointer(sample.object))
    {
        reason = "chat manager is unavailable";
        return false;
    }
    if (sample.vtable != kChatManagerVtable)
    {
        reason = "chat manager vtable mismatch";
        return false;
    }
    if (!valid_pointer(sample.member))
    {
        reason = "chat manager actor member is unavailable";
        return false;
    }
    return true;
}

bool compare_exact_prologue(std::span<const std::uint8_t> actual,
                            std::span<const std::uint8_t> expected)
{
    if (actual.size() != expected.size())
    {
        return false;
    }
    for (std::size_t index = 0; index < actual.size(); ++index)
    {
        if (actual[index] != expected[index])
        {
            return false;
        }
    }
    return true;
}

bool is_supported_client_name(std::wstring_view filename)
{
    for (const wchar_t* candidate : kSupportedClientNames)
    {
        const std::wstring_view expected(candidate);
        if (expected.size() != filename.size())
        {
            continue;
        }
        bool equal = true;
        for (std::size_t index = 0; index < expected.size(); ++index)
        {
            const wchar_t actual = filename[index];
            const wchar_t lowered = actual >= L'A' && actual <= L'Z' ? actual + (L'a' - L'A') : actual;
            if (lowered != expected[index])
            {
                equal = false;
                break;
            }
        }
        if (equal)
        {
            return true;
        }
    }
    return false;
}

void mask_launcher_patch_slots(std::span<std::uint8_t> image)
{
    for (const PatchSlot& slot : kLauncherPatchSlots)
    {
        if (slot.offset > image.size() || slot.length > image.size() - slot.offset)
        {
            continue;
        }
        for (std::size_t index = 0; index < slot.length; ++index)
        {
            image[slot.offset + index] = 0;
        }
    }
}

} // namespace navmut::native
