cmake_minimum_required(VERSION 3.13)
set(HPM_BUILD_TYPE ram)
find_package(hpm-sdk REQUIRED HINTS $ENV{HPM_SDK_BASE})
project(phy_diag)
sdk_app_src(phy_diag.c)

sdk_compile_definitions(-DCONFIG_NDEBUG_CONSOLE=1 -DBOARD_SHOW_CLOCK=0 -DBOARD_SHOW_BANNER=0)
