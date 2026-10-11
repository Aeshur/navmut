#include "navmut_native/protocol.h"
#include "navmut_native/pe_image.h"
#include "navmut_native/ipc_names.h"
#include "navmut_native/shared_state.h"
#include "navmut_native/validation.h"

#ifdef _WIN32
#include "navmut_native/hook_context.h"
#endif

#ifdef NDEBUG
#undef NDEBUG
#endif
#include <array>
#include <cassert>
#include <cmath>
#include <cstdint>
#include <iostream>
#include <string>
#include <vector>

namespace
{

void expect_error(std::string_view line, std::string_view fragment)
{
    const auto result = navmut::native::parse_protocol_line(line);
    assert(result.kind == navmut::native::ParseKind::Error);
    assert(result.reason.find(fragment) != std::string::npos);
}

void test_protocol()
{
    const auto valid = navmut::native::parse_protocol_line("POS 1.25 -2 0 42\r");
    assert(valid.kind == navmut::native::ParseKind::Position);
    assert(valid.position.zone == 42);
    assert(std::abs(valid.position.x - 1.25) < 1e-12);
    assert(valid.position.retail_command == "!pos 1.250 -2.000 0.000 42");
    assert(navmut::native::validate_retail_position(valid.position.retail_command));
    assert(!navmut::native::validate_retail_position("!pos 1 -2 0 42"));
    assert(!navmut::native::validate_retail_position("POS 1.250 -2.000 0.000 42"));
    for (const auto line : {"POS -0.0004 -0.0 0.0004 42", "POS -0.000 -0 -0.00049 42"})
    {
        const auto near_zero = navmut::native::parse_protocol_line(line);
        assert(near_zero.kind == navmut::native::ParseKind::Position);
        assert(near_zero.position.retail_command == "!pos 0.000 0.000 0.000 42");
        assert(navmut::native::validate_retail_position(near_zero.position.retail_command));
    }
    const auto negative = navmut::native::parse_protocol_line("POS -0.0006 0 0 42");
    assert(negative.position.retail_command == "!pos -0.001 0.000 0.000 42");
    assert(navmut::native::validate_retail_position(negative.position.retail_command));

    assert(navmut::native::parse_protocol_line("QUIT").kind == navmut::native::ParseKind::Quit);
    expect_error("", "expected");
    expect_error("POS 1 2 3", "expected");
    expect_error("POS 1 2 3 4 extra", "expected");
    expect_error("POS nan 2 3 4", "finite");
    expect_error("POS 1 inf 3 4", "finite");
    expect_error("POS 1000001 2 3 4", "outside");
    expect_error("POS 1 -1000001 3 4", "outside");
    expect_error("POS 1 2 3 0", "zone");
    expect_error("POS 1 2 3 65536", "zone");
    expect_error("QUIT now", "expected");
    expect_error("POS 1 2 3 4 \xC2", "ASCII");
    expect_error(std::string(257, 'x'), "too long");
}

void test_validation()
{
    std::string reason;
    const navmut::native::SceneChainSample scene{
        0x100000, navmut::native::kSceneVtable, 0x200000, 0x200010, 0x300000,
        navmut::native::kPlayerActorVtable,
    };
    assert(navmut::native::validate_scene_chain(scene, reason));
    auto wrong_scene = scene;
    wrong_scene.scene_vtable = 0;
    assert(!navmut::native::validate_scene_chain(wrong_scene, reason));
    auto wrong_container = scene;
    wrong_container.container = 0x200064;
    assert(!navmut::native::validate_scene_chain(wrong_container, reason));

    const navmut::native::ChatManagerSample chat{0x400000, navmut::native::kChatManagerVtable, 0x500000};
    assert(navmut::native::validate_chat_manager(chat, reason));
    auto no_member = chat;
    no_member.member = 0;
    assert(!navmut::native::validate_chat_manager(no_member, reason));

    const std::array<std::uint8_t, 4> bytes{0x55, 0x8B, 0xEC, 0x83};
    assert(navmut::native::compare_exact_prologue(bytes, bytes));
    auto changed = bytes;
    changed[2] = 0x90;
    assert(!navmut::native::compare_exact_prologue(changed, bytes));
}

void test_client_identity()
{
    assert(navmut::native::is_supported_client_name(L"ffxivgame.exe"));
    assert(navmut::native::is_supported_client_name(L"FFXIVGAME.EXE"));
    assert(navmut::native::is_supported_client_name(L"ffxivgame.patched.exe"));
    assert(navmut::native::is_supported_client_name(L"FFXIVGame.Patched.EXE"));
    assert(!navmut::native::is_supported_client_name(L"ffxivboot.exe"));
    assert(!navmut::native::is_supported_client_name(L"ffxivgame.patched.exe.bak"));

    std::vector<std::uint8_t> image(static_cast<std::size_t>(navmut::native::kSupportedClientSize));
    for (std::size_t index = 0; index < image.size(); ++index)
    {
        image[index] = static_cast<std::uint8_t>(index % 251);
    }
    auto patched = image;
    for (const auto& slot : navmut::native::kLauncherPatchSlots)
    {
        for (std::size_t index = 0; index < slot.length; ++index)
        {
            patched[slot.offset + index] = static_cast<std::uint8_t>(~patched[slot.offset + index]);
        }
    }
    navmut::native::mask_launcher_patch_slots(image);
    navmut::native::mask_launcher_patch_slots(patched);
    assert(image == patched);
    const auto& first = navmut::native::kLauncherPatchSlots[0];
    assert(image[first.offset] == 0);
    assert(image[first.offset + first.length - 1] == 0);
    assert(image[first.offset - 1] == static_cast<std::uint8_t>((first.offset - 1) % 251));
    assert(image[first.offset + first.length] == static_cast<std::uint8_t>((first.offset + first.length) % 251));

    std::vector<std::uint8_t> tiny(16, 7);
    navmut::native::mask_launcher_patch_slots(tiny);
    assert(tiny == std::vector<std::uint8_t>(16, 7));
}

void test_shared_cleanup()
{
    navmut::native::SharedBlock block;
    navmut::native::initialise_shared_block(block, 1234, 5678, 9012, 3456, 9);
    assert(block.nonce_low == 5678);
    assert(block.nonce_high == 9012);
    assert(navmut::native::shared_header_valid(block, 1234, 5678, 9012));
    assert(!navmut::native::shared_header_valid(block, 1234, 5678, 9013));
    assert(!navmut::native::shared_header_valid(block, 1235, 5678, 9012));
    block.target_window = 0;
    assert(!navmut::native::shared_header_valid(block, 1234, 5678, 9012));
    block.target_window = 9;
    assert(navmut::native::ipc_mapping_name(1234, 5678, 9012)
           != navmut::native::ipc_mapping_name(1234, 5678, 9013));
    assert(navmut::native::ipc_request_event_name(1234, 5678, 9012)
           != navmut::native::ipc_request_event_name(1235, 5678, 9012));
    assert(block.request_state == static_cast<std::int32_t>(navmut::native::RequestState::Idle));
    assert(block.hook_ready == 0);
    const std::string longest(navmut::native::kSharedCommandBytes - 1, 'x');
    assert(navmut::native::set_shared_command(block, longest));
    assert(block.command_length == longest.size());
    assert(std::string(block.command) == longest);
    assert(!navmut::native::set_shared_command(block, std::string(navmut::native::kSharedCommandBytes, 'y')));
    assert(!navmut::native::set_shared_command(block, std::string(navmut::native::kSharedCommandBytes + 1, 'z')));
    assert(block.command_length == longest.size());
    assert(std::string(block.command) == longest);
    assert(navmut::native::set_shared_response(block, 7, navmut::native::ResponseCode::Ok, "ok"));
    assert(block.response_sequence == 7);
    assert(block.response_code == static_cast<std::int32_t>(navmut::native::ResponseCode::Ok));
    assert(block.request_state == static_cast<std::int32_t>(navmut::native::RequestState::Complete));
    assert(std::string(block.response, block.response_length) == "ok");
    assert(!navmut::native::set_shared_response(block, 8, navmut::native::ResponseCode::Error,
                                               std::string(256, 'x')));
}


void test_pe_image()
{
    std::vector<std::uint8_t> bytes(512);
    const auto write_u16 = [&](std::size_t offset, std::uint16_t value) {
        bytes[offset] = static_cast<std::uint8_t>(value);
        bytes[offset + 1] = static_cast<std::uint8_t>(value >> 8);
    };
    const auto write_u32 = [&](std::size_t offset, std::uint32_t value) {
        for (std::size_t index = 0; index < 4; ++index)
        {
            bytes[offset + index] = static_cast<std::uint8_t>(value >> (index * 8));
        }
    };
    write_u16(0, 0x5A4D);
    write_u32(0x3C, 0x80);
    write_u32(0x80, 0x00004550);
    write_u16(0x86, 1);
    write_u16(0x94, 64);
    write_u16(0x98, 0x10B);
    write_u32(0xB4, navmut::native::kImageBase);
    write_u32(0xD4, 0x100);
    write_u32(0xE4, 0x1000);
    write_u32(0xE8, 16);
    write_u32(0xEC, 0x100);
    bytes[0x100] = 0x55;
    navmut::native::PeImage image;
    std::string reason;
    assert(image.load(bytes, reason));
    bytes[0x100] = 0x90;
    std::array<std::uint8_t, 1> prologue{};
    assert(image.bytes_at_rva(0x1000, prologue));
    assert(prologue[0] == 0x55);

    for (const std::uint32_t offset : {0xFFFFFFF0U, 0xFFFFFFFFU, 500U})
    {
        write_u32(0x3C, offset);
        assert(!image.load(bytes, reason));
    }
    write_u32(0x3C, 0x80);
    write_u16(0x94, 0xFFFF);
    assert(!image.load(bytes, reason));
    write_u16(0x94, 2);
    assert(!image.load(bytes, reason));
    write_u16(0x94, 64);
    write_u16(0x86, 0xFFFF);
    assert(!image.load(bytes, reason));
    write_u16(0x86, 1);
    write_u32(0xEC, 0xFFFFFFF8);
    assert(!image.load(bytes, reason));
}

#ifdef _WIN32
struct HelperSessionObjects
{
    HANDLE mapping = nullptr;
    HANDLE request_event = nullptr;
    HANDLE response_event = nullptr;
    HANDLE stop_event = nullptr;
    navmut::native::SharedBlock* shared = nullptr;

    HelperSessionObjects(std::uint32_t process_id, std::uint32_t nonce_low, std::uint32_t nonce_high)
    {
        mapping = CreateFileMappingW(INVALID_HANDLE_VALUE, nullptr, PAGE_READWRITE, 0,
                                     sizeof(navmut::native::SharedBlock),
                                     navmut::native::ipc_mapping_name(process_id, nonce_low, nonce_high).c_str());
        request_event = CreateEventW(nullptr, FALSE, FALSE,
                                     navmut::native::ipc_request_event_name(process_id, nonce_low, nonce_high).c_str());
        response_event = CreateEventW(nullptr, FALSE, FALSE,
                                      navmut::native::ipc_response_event_name(process_id, nonce_low, nonce_high).c_str());
        stop_event = CreateEventW(nullptr, TRUE, FALSE,
                                  navmut::native::ipc_stop_event_name(process_id, nonce_low, nonce_high).c_str());
        assert(mapping != nullptr && request_event != nullptr && response_event != nullptr && stop_event != nullptr);
        shared = static_cast<navmut::native::SharedBlock*>(
            MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, sizeof(navmut::native::SharedBlock)));
        assert(shared != nullptr);
        navmut::native::initialise_shared_block(*shared, process_id, nonce_low, nonce_high, 1, 9);
    }

    ~HelperSessionObjects()
    {
        UnmapViewOfFile(shared);
        CloseHandle(stop_event);
        CloseHandle(response_event);
        CloseHandle(request_event);
        CloseHandle(mapping);
    }

    HelperSessionObjects(const HelperSessionObjects&) = delete;
    HelperSessionObjects& operator=(const HelperSessionObjects&) = delete;
};

void test_hook_context_reattach()
{
    const std::uint32_t process_id = GetCurrentProcessId();
    navmut::native::HookContext context;
    assert(!navmut::native::open_hook_context(context, process_id, 0, 0));
    assert(!navmut::native::open_hook_context(context, process_id, 11, 12));

    HelperSessionObjects first(process_id, 11, 12);
    assert(navmut::native::open_hook_context(context, process_id, 11, 12));
    assert(context.shared->nonce_low == 11);

    HelperSessionObjects second(process_id, 21, 22);
    assert(navmut::native::open_hook_context(context, process_id, 21, 22));
    assert(context.shared->nonce_low == 21);

    assert(!navmut::native::open_hook_context(context, process_id, 31, 32));
    assert(context.shared != nullptr && context.shared->nonce_low == 21);

    navmut::native::close_hook_context(context);
    assert(context.shared == nullptr && context.mapping == nullptr && context.stop_event == nullptr);
}
#endif

} // namespace

int main()
{
    test_protocol();
    test_validation();
    test_client_identity();
    test_shared_cleanup();
    test_pe_image();
#ifdef _WIN32
    test_hook_context_reattach();
#endif
    std::cout << "native protocol, validation, client identity, and cleanup tests passed\n";
    return 0;
}
