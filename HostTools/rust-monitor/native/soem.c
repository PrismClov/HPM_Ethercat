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

static int exchange(ecx_contextt *context, ec_slavet *slave)
{
    memset(slave->outputs, 0, HPM_PDO_BYTES);
    if (ecx_send_processdata(context) <= 0)
        return EC_NOFRAME;
    return ecx_receive_processdata(context, EC_TIMEOUTRET);
}

static int run_pdo(ecx_contextt *context, int count)
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
    (void)exchange(context, slave);
    slave->state = EC_STATE_OPERATIONAL;
    int result = 1;
    if (ecx_writestate(context, 1) <= 0)
    {
        pdo_error("Could not request OP from the HPM slave.");
        goto stop;
    }

    int state = -1, code = -1;
    for (unsigned attempt = 0; attempt < 300; ++attempt)
    {
        (void)exchange(context, slave);
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
        int wkc = exchange(context, slave);
        printf("ecat\tsnapshot\t%d\t%s\n", responding, responding ? "responding" : "silent");
        if (responding)
            slave_row(0, slave->eep_man, slave->eep_id, slave->eep_rev,
                      state, code, slave->name, sizeof(slave->name));
        pdo_row(responding && state == EC_STATE_OPERATIONAL && wkc == expected,
                wkc, expected, slave->inputs);
        end_snapshot();
    }
    result = 0;

stop:
    slave->state = EC_STATE_SAFE_OP;
    (void)ecx_writestate(context, 1);
    (void)ecx_statecheck(context, 1, EC_STATE_SAFE_OP, EC_TIMEOUTRET3);
    return result;
}

int main(int argc, char **argv)
{
    static ecx_contextt context;
    int pdo = argc == 3 && strcmp(argv[2], "--pdo") == 0;
    if ((argc != 2 && !pdo) || !argv[1][0])
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

    if (pdo)
    {
        int result = run_pdo(&context, count);
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
