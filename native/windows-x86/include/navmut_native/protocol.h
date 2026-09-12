#pragma once

#include <cstdint>
#include <string>
#include <string_view>

namespace navmut::native
{

inline constexpr std::size_t kMaxProtocolLine = 256;
inline constexpr std::size_t kMaxCommandBytes = 128;
inline constexpr double kCoordinateLimit = 1'000'000.0;

struct Position
{
    double x = 0.0;
    double y = 0.0;
    double z = 0.0;
    std::uint16_t zone = 0;
    std::string retail_command;
};

enum class ParseKind
{
    Position,
    Quit,
    Error,
};

struct ParseResult
{
    ParseKind kind = ParseKind::Error;
    Position position{};
    std::string reason;
};

ParseResult parse_protocol_line(std::string_view line);
std::string format_retail_position(double x, double y, double z, std::uint16_t zone);
bool validate_retail_position(std::string_view command);

} // namespace navmut::native
