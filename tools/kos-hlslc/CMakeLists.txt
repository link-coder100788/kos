# kos-hlslc: the HLSL compiler the kos-gl QEMU device runs (see main.cpp).
# Builds glslang and SPIRV-Cross from tools/ with only what it needs.
cmake_minimum_required(VERSION 3.20)
project(kos_hlslc C CXX)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)

set(ENABLE_OPT OFF CACHE BOOL "" FORCE)
set(GLSLANG_TESTS OFF CACHE BOOL "" FORCE)
set(GLSLANG_ENABLE_INSTALL OFF CACHE BOOL "" FORCE)
set(ENABLE_GLSLANG_BINARIES OFF CACHE BOOL "" FORCE)
set(BUILD_EXTERNAL OFF CACHE BOOL "" FORCE)
add_subdirectory(${CMAKE_CURRENT_SOURCE_DIR}/../glslang glslang EXCLUDE_FROM_ALL)

set(SPIRV_CROSS_CLI OFF CACHE BOOL "" FORCE)
set(SPIRV_CROSS_ENABLE_TESTS OFF CACHE BOOL "" FORCE)
set(SPIRV_CROSS_ENABLE_HLSL OFF CACHE BOOL "" FORCE)
set(SPIRV_CROSS_ENABLE_MSL OFF CACHE BOOL "" FORCE)
set(SPIRV_CROSS_ENABLE_CPP OFF CACHE BOOL "" FORCE)
set(SPIRV_CROSS_ENABLE_REFLECT OFF CACHE BOOL "" FORCE)
set(SPIRV_CROSS_ENABLE_C_API OFF CACHE BOOL "" FORCE)
set(SPIRV_CROSS_ENABLE_UTIL OFF CACHE BOOL "" FORCE)
set(SPIRV_CROSS_SKIP_INSTALL ON CACHE BOOL "" FORCE)
add_subdirectory(${CMAKE_CURRENT_SOURCE_DIR}/../spirv-cross spirv-cross EXCLUDE_FROM_ALL)

add_executable(kos-hlslc main.cpp)
target_link_libraries(kos-hlslc PRIVATE glslang::glslang glslang::glslang-default-resource-limits spirv-cross-glsl)
# ShaderLang.h only declares the HLSL settings with this.
target_compile_definitions(kos-hlslc PRIVATE ENABLE_HLSL)
