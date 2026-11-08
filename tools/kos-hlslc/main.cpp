// kos-hlslc: compiles a KOS program's HLSL vertex and pixel shaders to
// GLSL 4.10, for the kos-gl QEMU device to hand to OpenGL. glslang turns
// each shader into SPIR-V, and SPIRV-Cross turns that into GLSL.
//
// It reads, on stdin:
//
//     <vertex entry point>\n<pixel entry point>\n
//     <vertex source length>\n<vertex source><pixel source length>\n<pixel source>
//
// and writes, on stdout, either
//
//     ok\n<vertex GLSL length>\n<vertex GLSL><pixel GLSL length>\n<pixel GLSL>
//
// or `error\n` and the compiler's messages. It exits 0 either way; only a
// malformed request fails.
//
// What it arranges, so the device needn't look inside the shaders:
// - vertex inputs get the attribute locations the device feeds by
//   semantic: POSITION 0, COLOR 1, NORMAL 2, TEXCOORD0 3;
// - what the vertex shader passes to the pixel shader is matched up by
//   semantic, as in Direct3D (GLSL would match by name or location);
// - each texture is sampled through a GLSL sampler named
//   kos_t<register>_s<register>, from `Texture2D t : register(t1)` and
//   `SamplerState s : register(s0)`, so the device binds texture unit 1 to
//   kos_t1_s0;
// - each stage is GLSL for a separable program (the device joins them in
//   a program pipeline), so the two stages' globals never have to agree.
//   Globals and cbuffers become struct uniforms named kos_<cbuffer>, and
//   a global `float time` is the uniform kos_Global.time.

#include <glslang/Public/ResourceLimits.h>
#include <glslang/Public/ShaderLang.h>
#include <SPIRV/GlslangToSpv.h>
#include <spirv_glsl.hpp>

#include <algorithm>
#include <cctype>
#include <cstdio>
#include <iostream>
#include <iterator>
#include <map>
#include <set>
#include <string>
#include <vector>

namespace {

struct Failure {
    std::string message;
};

std::string upper(std::string s) {
    for (char& c : s) c = (char)std::toupper((unsigned char)c);
    return s;
}

/// The SPIR-V for one shader, or a Failure with glslang's messages.
std::vector<uint32_t> to_spirv(EShLanguage stage, const std::string& source, const std::string& entry) {
    glslang::TShader shader(stage);
    const char* text = source.c_str();
    int length = (int)source.size();
    const char* name = stage == EShLangVertex ? "vertex shader" : "pixel shader";
    shader.setStringsWithLengthsAndNames(&text, &length, &name, 1);
    shader.setEntryPoint(entry.c_str());
    shader.setSourceEntryPoint(entry.c_str());
    shader.setEnvInput(glslang::EShSourceHlsl, stage, glslang::EShClientVulkan, 100);
    shader.setEnvClient(glslang::EShClientVulkan, glslang::EShTargetVulkan_1_0);
    shader.setEnvTarget(glslang::EShTargetSpv, glslang::EShTargetSpv_1_0);
    // Keeps each input and output's semantic in the SPIR-V.
    shader.setEnvTargetHlslFunctionality1();
    shader.setAutoMapBindings(true);
    shader.setAutoMapLocations(true);
    auto messages = (EShMessages)(EShMsgSpvRules | EShMsgVulkanRules | EShMsgReadHlsl);
    if (!shader.parse(GetDefaultResources(), 100, false, messages)) {
        throw Failure{shader.getInfoLog()};
    }
    glslang::TProgram program;
    program.addShader(&shader);
    if (!program.link(messages) || !program.mapIO()) {
        throw Failure{program.getInfoLog()};
    }
    std::vector<uint32_t> spirv;
    glslang::GlslangToSpv(*program.getIntermediate(stage), spirv);
    return spirv;
}

std::string semantic(const spirv_cross::Compiler& c, spirv_cross::ID id) {
    return upper(c.get_decoration_string(id, spv::DecorationUserSemantic));
}

/// The attribute location the device feeds a vertex input semantic from.
int vertex_input_location(const std::string& sem) {
    if (sem == "POSITION" || sem == "POSITION0") return 0;
    if (sem == "COLOR" || sem == "COLOR0") return 1;
    if (sem == "NORMAL" || sem == "NORMAL0") return 2;
    if (sem == "TEXCOORD" || sem == "TEXCOORD0") return 3;
    throw Failure{"vertex shader: KOS has no vertex input " + sem +
                  " (it gives POSITION, COLOR, NORMAL and TEXCOORD0)\n"};
}

std::string to_glsl(std::vector<uint32_t> spirv, bool vertex, const std::map<std::string, int>& varyings) {
    spirv_cross::CompilerGLSL c(std::move(spirv));
    spirv_cross::ShaderResources resources = c.get_shader_resources();
    for (const auto& input : resources.stage_inputs) {
        std::string sem = semantic(c, input.id);
        int location = vertex ? vertex_input_location(sem) : varyings.at(sem);
        c.set_decoration(input.id, spv::DecorationLocation, location);
    }
    if (vertex) {
        for (const auto& output : resources.stage_outputs) {
            c.set_decoration(output.id, spv::DecorationLocation, varyings.at(semantic(c, output.id)));
        }
    }

    c.build_combined_image_samplers();
    for (const auto& remap : c.get_combined_image_samplers()) {
        char name[64];
        std::snprintf(name, sizeof name, "kos_t%u_s%u", c.get_decoration(remap.image_id, spv::DecorationBinding),
                      c.get_decoration(remap.sampler_id, spv::DecorationBinding));
        c.set_name(remap.combined_id, name);
    }
    for (const auto& block : resources.uniform_buffers) {
        c.set_name(block.id, "kos" + c.get_name(block.base_type_id));
    }
    // Values set with glUniform go in uniform blocks in SPIR-V; plain
    // uniforms need no buffer objects.
    spirv_cross::CompilerGLSL::Options options;
    options.version = 410;
    options.es = false;
    options.enable_420pack_extension = false;
    options.emit_uniform_buffer_as_plain_uniforms = true;
    options.separate_shader_objects = true;
    c.set_common_options(options);
    return c.compile();
}

/// The semantics passed from the vertex shader to the pixel shader, each
/// given a location (in sorted order, so both shaders agree).
std::map<std::string, int> varying_locations(const std::vector<uint32_t>& vs, const std::vector<uint32_t>& ps) {
    std::set<std::string> names;
    spirv_cross::Compiler v(vs), p(ps);
    for (const auto& output : v.get_shader_resources().stage_outputs) names.insert(semantic(v, output.id));
    for (const auto& input : p.get_shader_resources().stage_inputs) names.insert(semantic(p, input.id));
    std::map<std::string, int> locations;
    for (const auto& name : names) {
        int next = (int)locations.size();
        locations[name] = next;
    }
    return locations;
}

bool read_line(std::string& line) {
    return (bool)std::getline(std::cin, line);
}

bool read_block(std::string& out) {
    std::string line;
    if (!read_line(line)) return false;
    size_t length = std::stoul(line);
    out.resize(length);
    return length == 0 || (bool)std::cin.read(&out[0], (std::streamsize)length);
}

}  // namespace

int main() {
    std::string vs_entry, ps_entry, vs_source, ps_source;
    try {
        if (!read_line(vs_entry) || !read_line(ps_entry) || !read_block(vs_source) || !read_block(ps_source)) {
            std::fprintf(stderr, "kos-hlslc: malformed request\n");
            return 1;
        }
    } catch (const std::exception&) {
        std::fprintf(stderr, "kos-hlslc: malformed request\n");
        return 1;
    }

    glslang::InitializeProcess();
    std::string result;
    try {
        std::vector<uint32_t> vs = to_spirv(EShLangVertex, vs_source, vs_entry);
        std::vector<uint32_t> ps = to_spirv(EShLangFragment, ps_source, ps_entry);
        std::map<std::string, int> varyings = varying_locations(vs, ps);
        std::string vs_glsl = to_glsl(std::move(vs), true, varyings);
        std::string ps_glsl = to_glsl(std::move(ps), false, varyings);
        result = "ok\n" + std::to_string(vs_glsl.size()) + "\n" + vs_glsl +
                 std::to_string(ps_glsl.size()) + "\n" + ps_glsl;
    } catch (const Failure& f) {
        result = "error\n" + f.message;
    } catch (const spirv_cross::CompilerError& e) {
        result = std::string("error\n") + e.what() + "\n";
    } catch (const std::out_of_range&) {
        result = "error\nthe vertex and pixel shaders' semantics don't match\n";
    }
    glslang::FinalizeProcess();
    std::fwrite(result.data(), 1, result.size(), stdout);
    return 0;
}
