#include "sart/installer_backends.hpp"

#include <algorithm>
#include <array>
#include <stdexcept>
#include <zlib.h>
#include <zstd.h>

namespace sart::install {
    namespace {

        void append_bounded(std::vector<std::byte> &output, const std::byte *buffer, std::size_t count) {
            if (count > max_inspected_archive_bytes - output.size()) {
                throw std::runtime_error("decompressed boot-deploy archive exceeds its bound");
            }
            output.insert(output.end(), buffer, buffer + count);
        }

        std::vector<std::byte> inflate_gzip(std::span<const std::byte> candidate) {
            z_stream stream{};
            stream.next_in = reinterpret_cast<Bytef *>(const_cast<std::byte *>(candidate.data()));
            stream.avail_in = static_cast<uInt>(candidate.size());
            if (inflateInit2(&stream, 15 + 16) != Z_OK)
                throw std::runtime_error("gzip decoder initialization failed");
            struct End {
                z_stream *stream;
                ~End() { inflateEnd(stream); }
            } end{&stream};
            std::vector<std::byte> output;
            std::array<std::byte, 64 * 1024> buffer{};
            while (true) {
                stream.next_out = reinterpret_cast<Bytef *>(buffer.data());
                stream.avail_out = buffer.size();
                const int status = inflate(&stream, Z_NO_FLUSH);
                append_bounded(output, buffer.data(), buffer.size() - stream.avail_out);
                if (status == Z_STREAM_END) {
                    if (stream.avail_in != 0 || stream.total_in != candidate.size()) {
                        throw std::runtime_error("gzip archive has trailing or concatenated data");
                    }
                    break;
                }
                if (status != Z_OK || (stream.avail_in == 0 && stream.avail_out != 0)) {
                    throw std::runtime_error("gzip archive is truncated or invalid");
                }
            }
            return output;
        }

        std::vector<std::byte> inflate_zstd(std::span<const std::byte> candidate) {
            ZSTD_DCtx *context = ZSTD_createDCtx();
            if (context == nullptr)
                throw std::runtime_error("Zstandard decoder initialization failed");
            struct End {
                ZSTD_DCtx *context;
                ~End() { ZSTD_freeDCtx(context); }
            } end{context};
            const auto parameter = ZSTD_DCtx_setParameter(context, ZSTD_d_windowLogMax, 27);
            if (ZSTD_isError(parameter))
                throw std::runtime_error("Zstandard decoder window bound failed");
            ZSTD_inBuffer input{candidate.data(), candidate.size(), 0};
            std::vector<std::byte> output;
            std::array<std::byte, 64 * 1024> buffer{};
            while (true) {
                ZSTD_outBuffer block{buffer.data(), buffer.size(), 0};
                const auto remaining = ZSTD_decompressStream(context, &block, &input);
                if (ZSTD_isError(remaining))
                    throw std::runtime_error("Zstandard archive is invalid");
                append_bounded(output, buffer.data(), block.pos);
                if (remaining == 0) {
                    if (input.pos != input.size)
                        throw std::runtime_error("Zstandard archive has trailing or concatenated data");
                    break;
                }
                if (input.pos == input.size && block.pos == 0)
                    throw std::runtime_error("Zstandard archive is truncated");
            }
            return output;
        }

    } // namespace

    std::vector<std::byte> decompress_mkinitfs_boot_deploy_archive(std::span<const std::byte> candidate,
                                                                   MkinitfsBootDeployCompression expected) {
        if (candidate.empty() || candidate.size() > max_candidate_bytes) {
            throw std::runtime_error("compressed boot-deploy archive size is unsupported");
        }
        const auto detected = detect_mkinitfs_boot_deploy_compression(candidate);
        if (detected != expected)
            throw std::runtime_error("boot-deploy compression differs from discovery");
        auto output =
            expected == MkinitfsBootDeployCompression::gzip ? inflate_gzip(candidate) : inflate_zstd(candidate);
        if (output.empty())
            throw std::runtime_error("decompressed boot-deploy archive is empty");
        return output;
    }

} // namespace sart::install
