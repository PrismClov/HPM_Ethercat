#include <soem/soem.h>
#include <string.h>
#include "hpm_pdo.h"

static int bus_count(ecx_contextt *context)
{
    uint16_t value = 0;
    int wkc = ecx_BRD(&context->port, 0, ECT_REG_ALSTAT, sizeof(value), &value, EC_TIMEOUTRET);
    return wkc > 0 ? wkc : 0;
}

static void read_al(ecx_contextt *context, const ec_slavet *slave, int *state, int *code)
{
    uint16_t status[3] = {0};
    int wkc = ecx_FPRD(&context->port, slave->configadr, ECT_REG_ALSTAT,
                       sizeof(status), status, EC_TIMEOUTRET);
    *state = wkc == 1 ? etohs(status[0]) : -1;
    *code = wkc == 1 ? etohs(status[2]) : -1;
}

static size_t read_sdo(void *master, uint16_t index, uint8_t subindex,
                       uint8_t *bytes, size_t capacity)
{
    int size = (int)capacity;
    return ecx_SDOread(master, 1, index, subindex, FALSE, &size, bytes, EC_TIMEOUTRXM) > 0 &&
           size > 0 && (size_t)size <= capacity ? (size_t)size : 0;
}

static int exchange(ecx_contextt *context, ec_slavet *slave, uint32_t token, uint64_t *tx_ns)
{
    for (unsigned i = 0; i < HPM_PDO_BYTES; ++i)
        slave->outputs[i] = (uint8_t)(token >> (8u * i));
    if (tx_ns)
        *tx_ns = monotonic_ns();
    if (ecx_send_processdata(context) <= 0)
        return EC_NOFRAME;
    return ecx_receive_processdata(context, EC_TIMEOUTRET);
}

static int run_pdo(ecx_contextt *context, int count, int echo)
{
    ec_slavet *slave = &context->slavelist[1];
    if (count != 1 || slave->eep_man != HPM_VENDOR || slave->eep_id != HPM_PRODUCT)
    {
        pdo_error("PDO mode requires exactly one HPM5E31-KIT (00024121:00000001).");
        return 1;
    }

    slave->state = EC_STATE_PRE_OP;
    if (ecx_writestate(context, 1) <= 0 ||
        ecx_statecheck(context, 1, EC_STATE_PRE_OP, EC_TIMEOUTSTATE) != EC_STATE_PRE_OP ||
        !hpm_mapping_matches(context, read_sdo))
    {
        pdo_error("Cannot enter PRE-OP or verify the HPM PDO layout via CoE.");
        return 1;
    }

    /* The verified payload has 8 process-data bytes plus mailbox status. */
    uint8_t io_map[4096] = {0};
    int mapped = ecx_config_map_group(context, io_map, 0);
    ec_groupt *group = &context->grouplist[0];
    int expected = group->outputsWKC * 2 + group->inputsWKC;
    if (mapped <= 0 || (size_t)mapped > sizeof(io_map) || expected <= 0 ||
        slave->Ibits != 32 || slave->Obits != 32 || slave->Istartbit || slave->Ostartbit ||
        !slave->inputs || !slave->outputs)
    {
        pdo_error("SOEM did not map the expected byte-aligned 4-byte input and output.");
        return 1;
    }

    slave->state = EC_STATE_SAFE_OP;
    if (ecx_writestate(context, 1) <= 0 ||
        ecx_statecheck(context, 1, EC_STATE_SAFE_OP, EC_TIMEOUTSTATE) != EC_STATE_SAFE_OP)
    {
        pdo_error("HPM slave did not reach SAFE-OP.");
        return 1;
    }
    (void)exchange(context, slave, 0, NULL);
    slave->state = EC_STATE_OPERATIONAL;
    int result = 1;
    uint32_t next_token = 0;
    if (ecx_writestate(context, 1) <= 0)
    {
        pdo_error("Could not request OP from the HPM slave.");
        goto stop;
    }

    int state = -1, code = -1;
    for (unsigned attempt = 0; attempt < 300; ++attempt)
    {
        (void)exchange(context, slave, 0, NULL);
        if (attempt % 8 == 0)
        {
            read_al(context, slave, &state, &code);
            if (state == EC_STATE_OPERATIONAL)
                break;
        }
        osal_usleep(2000);
    }
    if (state != EC_STATE_OPERATIONAL)
    {
        printf("ecat\terror\tHPM slave did not reach OP (AL state %d, status code %d).\n", state, code);
        goto stop;
    }

    int responding = 1;
    uint64_t first_valid_ns = 0;
    for (int request = 'S'; request; request = next_request())
    {
        if (request == 'S')
        {
            responding = bus_count(context);
            if (responding > 1)
            {
                pdo_error("SOEM PDO topology changed. Only one HPM5E31-KIT is supported.");
                goto stop;
            }
            read_al(context, slave, &state, &code);
        }
        uint32_t sent = next_token;
        uint64_t tx_ns = 0;
        int wkc = exchange(context, slave, sent, echo ? &tx_ns : NULL);
        uint64_t rx_ns = echo ? monotonic_ns() : 0;
        int valid = responding && state == EC_STATE_OPERATIONAL && wkc == expected;
        if (echo && !sent && valid)
        {
            if ((hpm_u32(slave->inputs) & ~3u) == HPM_ECHO_IDLE)
                next_token = 1;
            else
            {
                if (!first_valid_ns)
                    first_valid_ns = rx_ns;
                if (rx_ns - first_valid_ns >= UINT64_C(250000000))
                {
                    pdo_error("HPM echo firmware signature missing. Only zero outputs were sent.");
                    goto stop;
                }
            }
        }
        else if (sent)
            ++next_token;
        printf("ecat\tsnapshot\t%d\t%s\n", responding, responding ? "responding" : "silent");
        if (responding)
            slave_row(0, slave->eep_man, slave->eep_id, slave->eep_rev,
                      state, code, slave->name, sizeof(slave->name));
        pdo_row(valid, wkc, expected, slave->inputs);
        if (echo)
            echo_row(sent, tx_ns, rx_ns);
        end_snapshot();
    }
    result = 0;

stop:
    if (echo && next_token)
        (void)exchange(context, slave, 0, NULL);
    slave->state = EC_STATE_SAFE_OP;
    (void)ecx_writestate(context, 1);
    (void)ecx_statecheck(context, 1, EC_STATE_SAFE_OP, EC_TIMEOUTRET3);
    return result;
}

#include "soem_process.h"
#include "soem_eeprom.h"

int main(int argc, char **argv)
{
    if (argc == 2 && strcmp(argv[1], "--list-adapters") == 0)
    {
        ec_adaptert *adapters = ec_find_adapters();
        for (const ec_adaptert *adapter = adapters; adapter; adapter = adapter->next)
        {
            if (!adapter->name[0])
                continue;
            fputs("ecat\tadapter\t", stdout);
            hex_field(adapter->name, sizeof(adapter->name));
            putchar('\t');
            hex_field(adapter->desc, sizeof(adapter->desc));
            putchar('\n');
        }
        ec_free_adapters(adapters);
        end_snapshot();
        return 0;
    }
    static ecx_contextt context;
    int echo = argc == 3 && strcmp(argv[2], "--pdo-echo") == 0;
    int pdo = echo || (argc == 3 && strcmp(argv[2], "--pdo") == 0);
    uint32_t period = 0;
    unsigned assign = 0;
    int process = image_options(argc, argv, &period, &assign);
    unsigned eeprom_pos;
    char extra;
    int eeprom = argc == 4 && strcmp(argv[2], "--eeprom") == 0 &&
                 sscanf(argv[3], "%u %c", &eeprom_pos, &extra) == 1;
    if ((argc != 2 && !pdo && !process && !eeprom) || !argv[1][0])
        return 2;

    if (!ecx_init(&context, argv[1]))
    {
        puts("ecat\terror\tSOEM could not open the interface. Check the interface name, raw socket permissions or Npcap.");
        return 1;
    }

    if (pdo && !next_request())
    {
        ecx_close(&context);
        return 0;
    }
    if (pdo && bus_count(&context) != 1)
    {
        pdo_error("PDO mode requires exactly one responding slave before configuration.");
        ecx_close(&context);
        return 1;
    }

    /* Discover and assign station addresses without requesting PRE-OP. */
    context.manualstatechange = 1;
    int count = ecx_config_init(&context);
    if (count == EC_NOFRAME)
        count = 0;
    if (count < 0)
    {
        puts("ecat\terror\tSOEM discovery failed. Check the interface and bus size.");
        ecx_close(&context);
        return 1;
    }

    if (pdo || process || eeprom)
    {
        int result = eeprom ? soem_eeprom(&context, count, eeprom_pos)
            : process ? soem_process(&context, count, period, assign)
            : run_pdo(&context, count, echo);
        ecx_close(&context);
        return result;
    }

    int topology_changed = 0;
    while (next_request())
    {
        int responding = bus_count(&context);
        if (responding != count)
            topology_changed = 1;
        if (topology_changed)
        {
            printf("ecat\tsnapshot\t%d\tchanged\n", responding);
            end_snapshot();
            continue;
        }

        printf("ecat\tsnapshot\t%d\t%s\n", responding, responding ? "responding" : "silent");
        for (int i = 1; i <= count; ++i)
        {
            const ec_slavet *slave = &context.slavelist[i];
            int state, code;
            read_al(&context, slave, &state, &code);
            slave_row((unsigned)(i - 1), slave->eep_man, slave->eep_id, slave->eep_rev,
                      state, code, slave->name, sizeof(slave->name));
        }
        end_snapshot();
    }
    ecx_close(&context);
    return 0;
}
