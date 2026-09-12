#pragma once

#include <array>
#include <cstddef>
#include <cstdint>

namespace navmut::native
{

inline constexpr std::uint64_t kSupportedClientSize = 15'996'808ULL;
inline constexpr char kSupportedClientSha256[] =
    "9341f2b4567440b310a4d494f5cc5599ca334ba51c8042247317ff466492f2e9";

inline constexpr std::uint32_t kImageBase = 0x00400000U;
inline constexpr std::uint32_t kImageSize = 0x00F99000U;
inline constexpr std::uint32_t kSceneVtable = 0x00F8CC1CU;
inline constexpr std::uint32_t kChatManagerVtable = 0x00F912E4U;
inline constexpr std::uint32_t kPlayerActorVtable = 0x00FA7C50U;
inline constexpr std::uint32_t kSceneOuterOffset = 0x64U;
inline constexpr std::uint32_t kContainerActorOffset = 0x17838U;
inline constexpr std::uint32_t kChatManagerMemberOffset = 0x174ECU;

inline constexpr std::uint32_t kUtf8Constructor = 0x00445CF0U;
inline constexpr std::uint32_t kUtf8Assign = 0x004489C0U;
inline constexpr std::uint32_t kUtf8Destructor = 0x00446F50U;
inline constexpr std::uint32_t kChatEncoder = 0x004D8160U;
inline constexpr std::size_t kPrologueBytes = 16;
inline constexpr std::array<std::uint8_t, kPrologueBytes> kUtf8ConstructorPrologue{
    0x8B, 0xC1, 0xB9, 0x01, 0x00, 0x00, 0x00, 0x88,
    0x48, 0x10, 0x88, 0x48, 0x11, 0x89, 0x48, 0x08,
};
inline constexpr std::array<std::uint8_t, kPrologueBytes> kUtf8AssignPrologue{
    0x53, 0x8B, 0x5C, 0x24, 0x08, 0x56, 0x8B, 0xC3,
    0x57, 0x8B, 0xF9, 0x8D, 0x50, 0x01, 0x8B, 0xFF,
};
inline constexpr std::array<std::uint8_t, kPrologueBytes> kUtf8DestructorPrologue{
    0x80, 0x79, 0x11, 0x00, 0x75, 0x11, 0x8B, 0x41,
    0x04, 0x8B, 0x09, 0x6A, 0x0B, 0x50, 0x51, 0xE8,
};
inline constexpr std::array<std::uint8_t, kPrologueBytes> kChatEncoderPrologue{
    0x55, 0x8B, 0xEC, 0x83, 0xE4, 0xF0, 0x6A, 0xFF,
    0x68, 0x31, 0xD4, 0xE5, 0x00, 0x64, 0xA1, 0x00,
};

inline constexpr std::uint32_t kSharedMagic = 0x54414B4FU;
inline constexpr std::uint32_t kSharedVersion = 4;
inline constexpr std::size_t kSharedCommandBytes = 128;
inline constexpr std::uint32_t kWakeMessage = 0x8000U + 0x0231U;

enum class RequestState : std::int32_t
{
    Idle = 0,
    Pending = 1,
    Processing = 2,
    Complete = 3,
    Abandoned = 4,
};

enum class ResponseCode : std::int32_t
{
    None = 0,
    Ok = 1,
    Error = 2,
    Unknown = 3,
};

// This structure is shared by one helper and one injected x86 DLL. It is
// deliberately made entirely of fixed-width fields: no CRT objects, handles,
// pointers, or process-local synchronization primitives cross the mapping.
#pragma pack(push, 4)
struct SharedBlock
{
    std::uint32_t magic = 0;
    std::uint32_t version = 0;
    std::uint32_t size = 0;
    std::uint32_t target_pid = 0;
    std::uint32_t nonce_low = 0;
    std::uint32_t nonce_high = 0;
    std::uint32_t target_thread = 0;
    std::uint32_t target_window = 0;
    std::uint32_t scene_address = 0;
    std::uint32_t scene_outer = 0;
    std::uint32_t actor = 0;
    std::uint32_t chat_manager = 0;
    std::uint32_t actor_vtable = 0;
    std::uint32_t request_sequence = 0;
    std::uint32_t response_sequence = 0;
    volatile std::int32_t hook_ready = 0;
    volatile std::int32_t request_state = static_cast<std::int32_t>(RequestState::Idle);
    volatile std::int32_t response_code = static_cast<std::int32_t>(ResponseCode::None);
    std::uint32_t command_length = 0;
    char command[kSharedCommandBytes]{};
    std::uint32_t response_length = 0;
    char response[256]{};
};
#pragma pack(pop)

static_assert(sizeof(SharedBlock) < 1024, "shared state must remain small");

} // namespace navmut::native
