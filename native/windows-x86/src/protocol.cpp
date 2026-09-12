#include "navmut_native/protocol.h"

#include <charconv>
#include <cmath>
#include <cctype>
#include <iomanip>
#include <limits>
#include <locale>
#include <sstream>
#include <string>
#include <system_error>
#include <vector>

namespace navmut::native
{
namespace
{

bool is_ascii(std::string_view value)
{
    for (unsigned char character : value)
    {
        if (character > 0x7FU)
        {
            return false;
        }
    }
    return true;
}

std::vector<std::string_view> split_ascii_words(std::string_view value)
{
    std::vector<std::string_view> words;
    std::size_t cursor = 0;
    while (cursor < value.size())
    {
        while (cursor < value.size() && (value[cursor] == ' ' || value[cursor] == '\t'))
        {
            ++cursor;
        }
        if (cursor == value.size())
        {
            break;
        }
        const std::size_t start = cursor;
        while (cursor < value.size() && value[cursor] != ' ' && value[cursor] != '\t')
        {
            ++cursor;
        }
        words.emplace_back(value.substr(start, cursor - start));
    }
    return words;
}

enum class NumberResult
{
    Good,
    Malformed,
    NonFinite,
    OutOfRange,
};

NumberResult parse_coordinate(std::string_view token, double& value)
{
    const auto [end, error] = std::from_chars(token.data(), token.data() + token.size(), value,
                                               std::chars_format::general);
    if (error == std::errc::result_out_of_range)
    {
        return NumberResult::NonFinite;
    }
    if (error != std::errc{} || end != token.data() + token.size())
    {
        // Some standard libraries reject nan/inf in from_chars, while others
        // accept them.  Keep the protocol error stable for either behavior.
        std::string lowered(token);
        for (char& character : lowered)
        {
            character = static_cast<char>(std::tolower(static_cast<unsigned char>(character)));
        }
        if (lowered == "nan" || lowered == "+nan" || lowered == "-nan" || lowered == "inf"
            || lowered == "+inf" || lowered == "-inf" || lowered == "infinity"
            || lowered == "+infinity" || lowered == "-infinity")
        {
            return NumberResult::NonFinite;
        }
        return NumberResult::Malformed;
    }
    if (!std::isfinite(value))
    {
        return NumberResult::NonFinite;
    }
    if (value < -kCoordinateLimit || value > kCoordinateLimit)
    {
        return NumberResult::OutOfRange;
    }
    return NumberResult::Good;
}

bool parse_zone(std::string_view token, std::uint16_t& zone)
{
    std::uint32_t parsed = 0;
    const auto [end, error] = std::from_chars(token.data(), token.data() + token.size(), parsed, 10);
    if (error != std::errc{} || end != token.data() + token.size() || parsed == 0 || parsed > 65535U)
    {
        return false;
    }
    zone = static_cast<std::uint16_t>(parsed);
    return true;
}

ParseResult error_result(std::string reason)
{
    ParseResult result;
    result.kind = ParseKind::Error;
    result.reason = std::move(reason);
    return result;
}

} // namespace

std::string format_retail_position(double x, double y, double z, std::uint16_t zone)
{
    std::ostringstream stream;
    stream.imbue(std::locale::classic());
    stream << "!pos " << std::fixed << std::setprecision(3)
           << (x == 0.0 ? 0.0 : x) << ' ' << (y == 0.0 ? 0.0 : y) << ' '
           << (z == 0.0 ? 0.0 : z) << ' ' << zone;
    return stream.str();
}

ParseResult parse_protocol_line(std::string_view line)
{
    if (line.size() > kMaxProtocolLine)
    {
        return error_result("line is too long");
    }
    if (!line.empty() && line.back() == '\r')
    {
        line.remove_suffix(1);
    }
    if (!is_ascii(line))
    {
        return error_result("input must be ASCII");
    }
    const std::vector<std::string_view> words = split_ascii_words(line);
    if (words.size() == 1 && words[0] == "QUIT")
    {
        ParseResult result;
        result.kind = ParseKind::Quit;
        return result;
    }
    if (words.size() != 5 || words[0] != "POS")
    {
        return error_result("expected POS x y z zone or QUIT");
    }

    Position position;
    const NumberResult x_result = parse_coordinate(words[1], position.x);
    const NumberResult y_result = parse_coordinate(words[2], position.y);
    const NumberResult z_result = parse_coordinate(words[3], position.z);
    for (NumberResult result : {x_result, y_result, z_result})
    {
        if (result == NumberResult::NonFinite)
        {
            return error_result("coordinates must be finite");
        }
        if (result == NumberResult::OutOfRange)
        {
            return error_result("coordinate is outside [-1000000,1000000]");
        }
        if (result == NumberResult::Malformed)
        {
            return error_result("coordinate is malformed");
        }
    }
    if (!parse_zone(words[4], position.zone))
    {
        return error_result("zone must be an integer from 1 through 65535");
    }
    position.retail_command = format_retail_position(position.x, position.y, position.z, position.zone);
    ParseResult result;
    result.kind = ParseKind::Position;
    result.position = std::move(position);
    return result;
}

bool validate_retail_position(std::string_view command)
{
    constexpr std::string_view prefix = "!pos ";
    if (!command.starts_with(prefix))
    {
        return false;
    }
    std::string protocol_line = "POS ";
    protocol_line.append(command.substr(prefix.size()));
    const ParseResult parsed = parse_protocol_line(protocol_line);
    return parsed.kind == ParseKind::Position && parsed.position.retail_command == command;
}

} // namespace navmut::native
