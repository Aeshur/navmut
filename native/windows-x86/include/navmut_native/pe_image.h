#pragma once

#include "navmut_native/retail_layout.h"

#include <cstdint>
#include <cstring>
#include <span>
#include <string>
#include <utility>
#include <vector>

namespace navmut::native
{

class PeImage
{
public:
    bool load(std::vector<std::uint8_t> bytes, std::string& reason)
    {
        bytes_ = std::move(bytes);
        std::uint16_t dos_magic = 0;
        std::uint32_t pe_offset = 0;
        if (!read_u16(bytes_, 0, dos_magic) || dos_magic != 0x5A4DU || !read_u32(bytes_, 0x3CU, pe_offset)
            || !contains(bytes_, pe_offset, 24))
        {
            reason = "the FFXIV executable has an invalid PE header";
            return false;
        }
        std::uint32_t signature = 0;
        if (!read_u32(bytes_, pe_offset, signature) || signature != 0x00004550U)
        {
            reason = "the FFXIV executable has an invalid PE signature";
            return false;
        }
        const std::size_t file_header = pe_offset + 4;
        std::uint16_t sections = 0;
        std::uint16_t optional_size = 0;
        if (!read_u16(bytes_, file_header + 2, sections) || !read_u16(bytes_, file_header + 16, optional_size))
        {
            reason = "the FFXIV executable has an invalid PE file header";
            return false;
        }
        const std::size_t optional = file_header + 20;
        if (!contains(bytes_, optional, optional_size) || optional_size < 64)
        {
            reason = "the FFXIV executable has an invalid PE optional header";
            return false;
        }
        std::uint16_t optional_magic = 0;
        if (!read_u16(bytes_, optional, optional_magic) || optional_magic != 0x10BU
            || !read_u32(bytes_, optional + 28, image_base_) || !read_u32(bytes_, optional + 60, headers_size_))
        {
            reason = "the FFXIV executable is not the supported 32-bit image";
            return false;
        }
        const std::size_t section_table = optional + optional_size;
        if (!contains(bytes_, section_table, static_cast<std::size_t>(sections) * 40))
        {
            reason = "the FFXIV executable has an invalid PE section table";
            return false;
        }
        sections_.clear();
        for (std::size_t index = 0; index < sections; ++index)
        {
            const std::size_t section = section_table + index * 40;
            Section value;
            if (!read_u32(bytes_, section + 12, value.virtual_address)
                || !read_u32(bytes_, section + 16, value.raw_size)
                || !read_u32(bytes_, section + 20, value.raw_address))
            {
                reason = "the FFXIV executable has an invalid PE section";
                return false;
            }
            if (value.raw_address > bytes_.size() || value.raw_size > bytes_.size() - value.raw_address)
            {
                reason = "the FFXIV executable has an invalid PE section range";
                return false;
            }
            sections_.push_back(value);
        }
        if (image_base_ != kImageBase)
        {
            reason = "the FFXIV executable image base does not match the supported client";
            return false;
        }
        return true;
    }

    [[nodiscard]] std::uint32_t image_base() const
    {
        return image_base_;
    }

    bool bytes_at_rva(std::uint32_t rva, std::span<std::uint8_t> destination) const
    {
        const std::uint64_t end = static_cast<std::uint64_t>(rva) + destination.size();
        if (rva < headers_size_ && end <= bytes_.size())
        {
            std::memcpy(destination.data(), bytes_.data() + rva, destination.size());
            return true;
        }
        for (const Section& section : sections_)
        {
            const std::uint64_t section_end = static_cast<std::uint64_t>(section.virtual_address) + section.raw_size;
            if (rva >= section.virtual_address && end <= section_end)
            {
                const std::uint64_t offset = static_cast<std::uint64_t>(section.raw_address)
                    + rva - section.virtual_address;
                if (offset + destination.size() <= bytes_.size())
                {
                    std::memcpy(destination.data(), bytes_.data() + offset, destination.size());
                    return true;
                }
            }
        }
        return false;
    }

private:
    static bool contains(const std::vector<std::uint8_t>& bytes, std::size_t offset, std::size_t length)
    {
        return offset <= bytes.size() && length <= bytes.size() - offset;
    }

    static bool read_u16(const std::vector<std::uint8_t>& bytes, std::size_t offset, std::uint16_t& value)
    {
        if (!contains(bytes, offset, sizeof(value)))
        {
            return false;
        }
        value = static_cast<std::uint16_t>(bytes[offset])
            | (static_cast<std::uint16_t>(bytes[offset + 1]) << 8);
        return true;
    }
    
    static bool read_u32(const std::vector<std::uint8_t>& bytes, std::size_t offset, std::uint32_t& value)
    {
        if (!contains(bytes, offset, sizeof(value)))
        {
            return false;
        }
        value = static_cast<std::uint32_t>(bytes[offset])
            | (static_cast<std::uint32_t>(bytes[offset + 1]) << 8)
            | (static_cast<std::uint32_t>(bytes[offset + 2]) << 16)
            | (static_cast<std::uint32_t>(bytes[offset + 3]) << 24);
        return true;
    }

    struct Section
    {
        std::uint32_t virtual_address = 0;
        std::uint32_t raw_size = 0;
        std::uint32_t raw_address = 0;
    };

    std::vector<std::uint8_t> bytes_;
    std::vector<Section> sections_;
    std::uint32_t image_base_ = 0;
    std::uint32_t headers_size_ = 0;
};

} // namespace navmut::native
