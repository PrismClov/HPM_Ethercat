#include <ecrt.h>
#include <errno.h>
#include <stdlib.h>
#include <time.h>
#include <string.h>
#include "hpm_pdo.h"

static void sleep_ms(long ms)
{
    const struct timespec delay = {.tv_nsec = ms * 1000000L};
    nanosleep(&delay, NULL);
}

static size_t read_sdo(void *master, uint16_t index, uint8_t subindex,
                       uint8_t *bytes, size_t capacity)
{
    for (unsigned attempt = 0; attempt < 3; ++attempt)
    {
        size_t size = 0;
        uint32_t abort_code = 0;
        if (ecrt_master_sdo_upload(master, 0, index, subindex, bytes, capacity,
                                   &size, &abort_code) == 0)
            return size;
        if (abort_code)
            break;
        if (attempt < 2)
            sleep_ms(50);
    }
    fprintf(stderr, "IgH could not read 0x%04x:%02x.\n", (unsigned)index, (unsigned)subindex);
    return 0;
}

static int wait_for_preop(ec_master_t *master, ec_slave_info_t *slave)
{
    /* Reservation requests PRE-OP asynchronously, including after a previous run. */
    for (unsigned attempt = 0; attempt < 30; ++attempt)
    {
        if (ecrt_master_get_slave(master, 0, slave) == 0 &&
            slave->al_state == EC_AL_STATE_PREOP && !slave->error_flag)
            return 1;
        sleep_ms(100);
    }
    return 0;
}

static int run_pdo(ec_master_t *master, int echo)
{
    ec_master_info_t info = {0};
    for (unsigned attempt = 0; attempt < 30; ++attempt)
    {
        if (ecrt_master(master, &info) < 0)
        {
            pdo_error("Cannot read IgH master information.");
            return 1;
        }
        if (info.link_up && !info.scan_busy && info.slave_count)
            break;
        sleep_ms(100);
    }

    ec_slave_info_t slave = {0};
    if (!info.link_up || info.scan_busy || info.slave_count != 1 ||
        ecrt_master_get_slave(master, 0, &slave) < 0 ||
        slave.vendor_id != HPM_VENDOR || slave.product_code != HPM_PRODUCT)
    {
        pdo_error("PDO mode requires exactly one HPM5E31-KIT (00024121:00000001).");
        return 1;
    }
    if (ecrt_master_reserve(master) < 0)
    {
        pdo_error("Cannot reserve IgH master. Stop the other PDO application first.");
        return 1;
    }
    if (ecrt_master(master, &info) < 0 || info.slave_count != 1 ||
        !wait_for_preop(master, &slave) ||
        slave.vendor_id != HPM_VENDOR || slave.product_code != HPM_PRODUCT)
    {
        pdo_error("HPM slave did not reach PRE-OP after IgH master reservation.");
        return 1;
    }
    if (!hpm_mapping_matches(master, read_sdo))
    {
        pdo_error("Cannot verify HPM PDO assignments and 32-bit mappings via CoE.");
        return 1;
    }

    ec_domain_t *domain = ecrt_master_create_domain(master);
    ec_slave_config_t *config = ecrt_master_slave_config(master, 0, 0, HPM_VENDOR, HPM_PRODUCT);
    if (!domain || !config)
    {
        pdo_error("Cannot create IgH PDO domain or slave configuration.");
        return 1;
    }
    ec_pdo_entry_info_t entries[] = {{HPM_OUTPUT, 0, 32}, {HPM_INPUT, 0, 32}};
    ec_pdo_info_t pdos[] = {{0x1600, 1, &entries[0]}, {0x1a00, 1, &entries[1]}};
    ec_sync_info_t syncs[] = {
        {2, EC_DIR_OUTPUT, 1, &pdos[0], EC_WD_DEFAULT},
        {3, EC_DIR_INPUT, 1, &pdos[1], EC_WD_DEFAULT},
        {.index = 0xff}
    };
    if (ecrt_slave_config_pdos(config, EC_END, syncs))
    {
        pdo_error("Cannot apply the HPM PDO layout in IgH.");
        return 1;
    }

    int output = ecrt_slave_config_reg_pdo_entry(config, HPM_OUTPUT, 0, domain, NULL);
    int input = ecrt_slave_config_reg_pdo_entry(config, HPM_INPUT, 0, domain, NULL);
    if (output < 0 || input < 0 || ecrt_master_activate(master) < 0)
    {
        pdo_error("Cannot register or activate the IgH PDO domain.");
        return 1;
    }
    uint8_t *data = ecrt_domain_data(domain);
    size_t size = ecrt_domain_size(domain);
    if (!data || size != 2u * HPM_PDO_BYTES || output == input ||
        (size_t)output + HPM_PDO_BYTES > size ||
        (size_t)input + HPM_PDO_BYTES > size)
    {
        pdo_error("IgH returned invalid process data offsets.");
        return 1;
    }

    int error_flag = 0;
    uint32_t token = 0;
    uint64_t first_valid_ns = 0;
    for (int request = 'S'; request; request = next_request())
    {
        ecrt_master_receive(master);
        uint64_t rx_ns = echo ? monotonic_ns() : 0;
        ecrt_domain_process(domain);
        ec_domain_state_t domain_state = {0};
        ec_slave_config_state_t state = {0};
        ecrt_domain_state(domain, &domain_state);
        ecrt_slave_config_state(config, &state);

        ec_master_state_t master_state = {0};
        if (request == 'S')
        {
            ecrt_master_state(master, &master_state);
            ec_slave_info_t live = {0};
            error_flag = ecrt_master_get_slave(master, 0, &live) == 0 && live.error_flag;
        }
        if (master_state.slaves_responding > 1)
        {
            pdo_error("IgH PDO topology changed. Only one HPM5E31-KIT is supported.");
            return 1;
        }

        printf("ecat\tsnapshot\t%u\t%s\n", state.online ? 1u : 0u,
               state.online ? "responding" : "silent");
        if (state.online)
            slave_row(0, slave.vendor_id, slave.product_code, slave.revision_number,
                      state.al_state | (error_flag ? 0x10 : 0), -1, slave.name, sizeof(slave.name));
        int valid = state.operational && !error_flag && domain_state.wc_state == EC_WC_COMPLETE;
        pdo_row(valid, (int)domain_state.working_counter, -1, data + input);

        if (echo && !token && valid)
        {
            if ((hpm_u32(data + input) & ~3u) == HPM_ECHO_IDLE)
                token = 1;
            else
            {
                if (!first_valid_ns)
                    first_valid_ns = rx_ns;
                if (rx_ns - first_valid_ns >= UINT64_C(250000000))
                {
                    pdo_error("HPM echo firmware signature missing. Only zero outputs were sent.");
                    return 1;
                }
            }
        }
        else if (echo && token)
            ++token;

        memset(data + output, 0, HPM_PDO_BYTES);
        if (token)
            EC_WRITE_U32(data + output, token);
        ecrt_domain_queue(domain);
        uint64_t tx_ns = echo ? monotonic_ns() : 0;
        ecrt_master_send(master);
        if (echo)
            echo_row(token, tx_ns, rx_ns);
        end_snapshot();
    }
    if (echo && token)
    {
        memset(data + output, 0, HPM_PDO_BYTES);
        ecrt_master_receive(master);
        ecrt_domain_queue(domain);
        ecrt_master_send(master);
    }
    return 0;
}

int main(int argc, char **argv)
{
    char *end;
    int echo = argc == 3 && strcmp(argv[2], "--pdo-echo") == 0;
    if ((argc != 2 && argc != 3) || argv[1][0] < '0' || argv[1][0] > '9' ||
        (argc == 3 && !echo && strcmp(argv[2], "--pdo") != 0))
        return 2;
    errno = 0;
    unsigned long index = strtoul(argv[1], &end, 10);
    if (*end || errno || index > UINT32_MAX)
        return 2;

    ec_master_t *master = ecrt_open_master((unsigned)index);
    if (!master)
    {
        printf("ecat\terror\tCannot open IgH master %lu. Check /dev/EtherCAT%lu and device permissions.\n", index, index);
        return 1;
    }

    if (argc == 3)
    {
        int result = next_request() ? run_pdo(master, echo) : 0;
        ecrt_release_master(master);
        return result;
    }

    while (next_request())
    {
        ec_master_info_t info = {0};
        int result = ecrt_master(master, &info);
        if (result < 0 || info.slave_count > UINT16_MAX + 1u)
        {
            printf("ecat\terror\tCannot read IgH master information (%d).\n", result);
            break;
        }

        printf("ecat\tsnapshot\t%u\t%s\n", info.slave_count,
               info.scan_busy ? "scanning" : (info.link_up ? "up" : "down"));
        for (unsigned i = 0; info.link_up && !info.scan_busy && i < info.slave_count; ++i)
        {
            ec_slave_info_t slave = {0};
            if (ecrt_master_get_slave(master, (uint16_t)i, &slave) < 0)
                continue;
            int state = slave.al_state | (slave.error_flag ? 0x10 : 0);
            slave_row(i, slave.vendor_id, slave.product_code, slave.revision_number,
                      state, -1, slave.name, sizeof(slave.name));
        }
        end_snapshot();
    }
    ecrt_release_master(master);
    return 0;
}
