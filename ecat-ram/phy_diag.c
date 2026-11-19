#include "board.h"
#include "hpm_gpio_drv.h"
#include "hpm_esc_regs.h"

volatile struct {
    uint32_t stage, esc_hz, core_id, status[4], value[4], dl_status;
} result __attribute__((section(".noncacheable.init"))) = {.stage = 0x50485900};

static void read_phy(unsigned slot, unsigned reg)
{
    uint16_t cs;
    for (unsigned i = 0; i < 10000; ++i) {
        if (!(HPM_ESC->MII_MNG_CS & 0x8000)) {
            HPM_ESC->PHY_ADDR = BOARD_ECAT_PHY_ADDR_OFFSET + BOARD_ECAT_PORT0_PHY_ADDR;
            HPM_ESC->PHY_REG_ADDR = reg;
            HPM_ESC->MII_MNG_CS = (HPM_ESC->MII_MNG_CS & 0xfe) | 0x100;
            for (unsigned j = 0; j < 10000; ++j) {
                cs = HPM_ESC->MII_MNG_CS;
                if (!(cs & 0x8000)) {
                    result.status[slot] = cs;
                    result.value[slot] = HPM_ESC->PHY_DATA;
                    return;
                }
            }
            break;
        }
    }
    result.status[slot] = 0xffffffff;
}

int main(void)
{
    board_init_pmp();
    result.stage = 1;
    /* Keep the current CPU clock and power settings for this PHY test. */
    result.stage = 2;
    board_init_ethercat(HPM_ESC);
    result.esc_hz = clock_get_frequency(clock_esc0);
    result.stage = 3;
    board_delay_ms(30);
    gpio_write_pin(BOARD_ECAT_PHY0_RESET_GPIO, BOARD_ECAT_PHY0_RESET_GPIO_PORT_INDEX,
                   BOARD_ECAT_PHY0_RESET_PIN_INDEX, 1);
    board_delay_ms(1500);
    result.stage = 4;
    result.core_id = HPM_ESC->TYPE;
    result.stage = 5;
    HPM_ESC->MIIM_PDI_ACC_STAT = 1;
    read_phy(0, 2);
    read_phy(1, 3);
    read_phy(2, 1);
    read_phy(3, 1);
    result.dl_status = HPM_ESC->ESC_DL_STAT;
    result.stage = 6;
    for (;;) { __asm volatile ("nop"); }
}
