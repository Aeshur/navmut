#include "navmut_native/protocol.h"
#include "navmut_native/ipc_names.h"
#include "navmut_native/shared_state.h"
#include "navmut_native/validation.h"

#ifdef NDEBUG
#undef NDEBUG
#endif
#include <array>
#include <cassert>
#include <cmath>
#include <cstdint>
#include <iostream>
#include <string>

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
    assert(navmut::native::set_shared_response(block, 7, navmut::native::ResponseCode::Ok, "ok"));
    assert(block.response_sequence == 7);
    assert(block.response_code == static_cast<std::int32_t>(navmut::native::ResponseCode::Ok));
    assert(block.request_state == static_cast<std::int32_t>(navmut::native::RequestState::Complete));
    assert(std::string(block.response, block.response_length) == "ok");
    assert(!navmut::native::set_shared_response(block, 8, navmut::native::ResponseCode::Error,
                                               std::string(256, 'x')));
}

} // namespace

int main()
{
    test_protocol();
    test_validation();
    test_shared_cleanup();
    std::cout << "native protocol, validation, and cleanup tests passed\n";
    return 0;
}
