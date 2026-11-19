#include <stdbool.h>
#include <stdint.h>

#include "board.h"
#include "digital_io.h"
#include "hpm_gpio_drv.h"
#include "oled.h"

#define HPM_ECHO_IDLE UINT32_C(0xeca74ca0)
#define HPM_ECHO_MAX_TOKEN UINT32_C(0x3fffffff)

#if !defined(HPM_PDO_ECHO)
static uint32_t output_map_cycle;
static bool output_map_valid;

static uint32_t cycle_count(void)
{
    uint32_t cycles;
    __asm volatile ("csrr %0, mcycle" : "=r"(cycles) :: "memory");
    return cycles;
}
#endif

void APPL_InputMapping(UINT16 *data)
{
#if defined(HPM_PDO_ECHO)
    uint32_t token = OutputCounter0x7010;
    uint32_t echo = token && token <= HPM_ECHO_MAX_TOKEN ? token << 2 : HPM_ECHO_IDLE;
    uint32_t input = echo | (InputCounter0x6000 & 3u);
#else
    uint32_t phase = 0xffffu;
    uint32_t now = cycle_count();
    if (output_map_valid) {
        uint32_t elapsed = (now - output_map_cycle) / (BOARD_CPU_FREQ / 1000000u);
        if (elapsed < 0xffffu)
            phase = elapsed;
    }
    uint32_t input = (phase << 16) | (InputCounter0x6000 & 0xffffu);
#endif
    MEMCPY(data, &input, sizeof(input));
}

void APPL_OutputMapping(UINT16 *data)
{
    MEMCPY(&OutputCounter0x7010, data, sizeof(OutputCounter0x7010));
#if !defined(HPM_PDO_ECHO)
    output_map_cycle = cycle_count();
    output_map_valid = true;
#endif
}

void APPL_Application(void)
{
    InputCounter0x6000 =
        (uint32_t)gpio_read_pin(BOARD_ECAT_IN1_GPIO, BOARD_ECAT_IN1_GPIO_PORT_INDEX, BOARD_ECAT_IN1_GPIO_PIN_INDEX) |
        ((uint32_t)gpio_read_pin(BOARD_ECAT_IN2_GPIO, BOARD_ECAT_IN2_GPIO_PORT_INDEX, BOARD_ECAT_IN2_GPIO_PIN_INDEX) << 1);

#if defined(HPM_PDO_ECHO)
    board_led_write(BOARD_LED_OFF_LEVEL);
#else
    static uint32_t displayed;
    uint32_t output = OutputCounter0x7010;
    if (output != displayed) {
        OLED_ShowNum(0, 32, output, 10, 16, 1);
        OLED_Refresh();
        displayed = output;
    }
    board_led_write(output ? BOARD_LED_ON_LEVEL : BOARD_LED_OFF_LEVEL);
#endif
}
