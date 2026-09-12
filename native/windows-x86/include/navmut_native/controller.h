#pragma once

#include "navmut_native/protocol.h"

#include <cstdint>
#include <string>

namespace navmut::native
{

enum class SubmitStatus
{
    Ok,
    Error,
    Unknown,
};

struct SubmitResult
{
    SubmitStatus status = SubmitStatus::Error;
    std::string reason;
};

class HelperController
{
public:
    HelperController();
    ~HelperController();

    HelperController(const HelperController&) = delete;
    HelperController& operator=(const HelperController&) = delete;

    bool start(std::uint32_t process_id, std::uint32_t window_handle, std::string& reason);
    SubmitResult submit(const Position& position);
    void stop() noexcept;

private:
    struct Impl;
    Impl* impl_ = nullptr;
};

} // namespace navmut::native
