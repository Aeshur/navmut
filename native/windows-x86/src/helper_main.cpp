#include "navmut_native/controller.h"

#include <climits>
#include <cstdint>
#include <iostream>
#include <limits>
#include <string>
#include <string_view>

#if defined(_WIN32)
#include <windows.h>
#endif

namespace navmut::native
{
namespace
{

bool parse_pid(std::wstring_view token, std::uint32_t& pid)
{
    if (token.empty())
    {
        return false;
    }
    std::uint64_t value = 0;
    for (wchar_t character : token)
    {
        if (character < L'0' || character > L'9')
        {
            return false;
        }
        value = value * 10U + static_cast<unsigned>(character - L'0');
        if (value > std::numeric_limits<std::uint32_t>::max())
        {
            return false;
        }
    }
    if (value == 0)
    {
        return false;
    }
    pid = static_cast<std::uint32_t>(value);
    return true;
}

void write_line(std::string_view line)
{
    std::cout << line << '\n' << std::flush;
}

bool read_bounded_line(std::string& line)
{
    line.clear();
    bool too_long = false;
    char character = 0;
    while (std::cin.get(character))
    {
        if (character == '\n')
        {
            if (too_long)
            {
                line.assign(kMaxProtocolLine + 1, 'x');
            }
            return true;
        }
        if (line.size() < kMaxProtocolLine)
        {
            line.push_back(character);
        }
        else
        {
            too_long = true;
        }
    }
    if (too_long)
    {
        line.assign(kMaxProtocolLine + 1, 'x');
        return true;
    }
    return !line.empty();
}

} // namespace
} // namespace navmut::native

#if defined(_WIN32)
int wmain(int argc, wchar_t* argv[])
{
    using namespace navmut::native;
    if (argc != 5 || std::wstring_view(argv[1]) != L"--pid"
        || std::wstring_view(argv[3]) != L"--hwnd")
    {
        write_line("ERROR usage: navmut-helper.exe --pid N --hwnd H");
        return 2;
    }
    std::uint32_t pid = 0;
    if (!parse_pid(argv[2], pid))
    {
        write_line("ERROR pid must be a positive 32-bit integer");
        return 2;
    }

    std::uint32_t window_handle = 0;
    std::string reason;
    if (!parse_pid(argv[4], window_handle))
    {
        write_line("ERROR hwnd must be a positive 32-bit integer");
        return 2;
    }

    HelperController controller;
    bool attached = false;
    try
    {
        attached = controller.start(pid, window_handle, reason);
    }
    catch (...)
    {
        reason = "could not attach to the FFXIV process";
    }
    if (!attached)
    {
        write_line(std::string("ERROR ") + (reason.empty() ? "could not attach to the FFXIV process" : reason));
        return 1;
    }
    write_line("READY");

    std::string line;
    while (read_bounded_line(line))
    {
        ParseResult parsed;
        try
        {
            parsed = parse_protocol_line(line);
        }
        catch (...)
        {
            write_line("ERROR could not validate the protocol line");
            continue;
        }
        if (parsed.kind == ParseKind::Error)
        {
            write_line(std::string("ERROR ") + parsed.reason);
            continue;
        }
        if (parsed.kind == ParseKind::Quit)
        {
            controller.stop();
            write_line("BYE");
            return 0;
        }
        SubmitResult result;
        try
        {
            result = controller.submit(parsed.position);
        }
        catch (...)
        {
            controller.stop();
            write_line("UNKNOWN the helper failed while waiting for the command result");
            return 1;
        }
        if (result.status == SubmitStatus::Ok)
        {
            write_line("OK");
        }
        else if (result.status == SubmitStatus::Unknown)
        {
            write_line(std::string("UNKNOWN ") + result.reason);
        }
        else
        {
            write_line(std::string("ERROR ") + result.reason);
        }
    }
    controller.stop();
    return 0;
}
#else
int main()
{
    return 2;
}
#endif
