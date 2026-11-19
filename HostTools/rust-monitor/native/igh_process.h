#ifndef MONITOR_IGH_PROCESS_H
#define MONITOR_IGH_PROCESS_H

#include "process_image.h"

static int igh_map_slave(ec_master_t *master, ec_domain_t *domain, process_image *image,
                         ec_slave_config_t *config, const ec_slave_info_t *slave)
{
    /* IgH supplies the active mapping and each entry's actual domain offset. */
    for (unsigned sm = 0; sm < slave->sync_count; ++sm)
    {
        ec_sync_info_t sync = {0};
        if (ecrt_master_get_sync_manager(master, slave->position, (uint8_t)sm, &sync))
            return 0;
        for (unsigned p = 0; p < sync.n_pdos; ++p)
        {
            ec_pdo_info_t pdo = {0};
            if (ecrt_master_get_pdo(master, slave->position, (uint8_t)sm, (uint16_t)p, &pdo))
                return 0;
            for (unsigned i = 0; i < pdo.n_entries; ++i)
            {
                ec_pdo_entry_info_t entry = {0};
                if (ecrt_master_get_pdo_entry(master, slave->position, (uint8_t)sm,
                                              (uint16_t)p, (uint16_t)i, &entry))
                    return 0;
                if (!entry.index)
                    continue;
                if (sync.dir != EC_DIR_INPUT && sync.dir != EC_DIR_OUTPUT)
                    return 0;
                unsigned bit;
                int offset = ecrt_slave_config_reg_pdo_entry_pos(config, (uint8_t)sm, p, i,
                                                                  domain, &bit);
                if (offset < 0 || !image_add(image, slave->position, sync.dir == EC_DIR_OUTPUT,
                                             pdo.index, entry.index, entry.subindex,
                                             entry.bit_length, (size_t)offset * 8 + bit, NULL))
                    return 0;
            }
        }
    }
    return 1;
}

static int igh_process(ec_master_t *master, uint32_t period_ns, unsigned assign)
{
    ec_master_info_t info = {0};
    for (unsigned trial = 0; trial < 40; ++trial)
    {
        if (ecrt_master(master, &info))
            break;
        if (info.link_up && !info.scan_busy && info.slave_count)
            break;
        sleep_ms(100);
    }
    if (!info.link_up || info.scan_busy || !info.slave_count || info.slave_count > 4096 ||
        (assign && (info.slave_count != 1 || (assign & 0x0700) != 0x0300)) ||
        ecrt_master_reserve(master) < 0)
    {
        pdo_error("IgH process mode requires an available scanned bus; DC requires one slave and SYNC0.");
        return 1;
    }
    unsigned count = info.slave_count;
    process_image *image = calloc(1, sizeof(*image));
    ec_slave_config_t **configs = calloc(count, sizeof(*configs));
    ec_domain_t *domain = ecrt_master_create_domain(master);
    int result = 1;
    if (!image || !configs || !domain)
        goto failed;
    ecrt_master_set_send_interval(master, period_ns / 1000u);
    for (unsigned i = 0; i < count; ++i)
    {
        ec_slave_info_t slave;
        if (ecrt_master_get_slave(master, (uint16_t)i, &slave))
            goto failed;
        configs[i] = ecrt_master_slave_config(master, 0, (uint16_t)i, slave.vendor_id, slave.product_code);
        if (!configs[i] || !igh_map_slave(master, domain, image, configs[i], &slave))
            goto failed;
        if (assign && ecrt_slave_config_dc(configs[i], (uint16_t)assign, period_ns, 0, 0, 0))
            goto failed;
    }
    if (assign)
        ecrt_master_application_time(master, monotonic_ns());
    if (ecrt_master_activate(master) < 0)
        goto failed;
    image->data = ecrt_domain_data(domain);
    image->bytes = ecrt_domain_size(domain);
    if (!image_layout(image))
        goto failed;
    int topology_ok = 0;
    for (int request; (request = image_next(image)) != 0;)
    {
        ecrt_master_receive(master);
        ecrt_domain_process(domain);
        ec_domain_state_t state = {0};
        ecrt_domain_state(domain, &state);
        if (request == 'S' || !image->valid)
        {
            ec_master_state_t bus = {0};
            ecrt_master_state(master, &bus);
            topology_ok = bus.slaves_responding == count && bus.link_up;
            for (unsigned i = 0; i < count; ++i)
            {
                ec_slave_info_t slave = {0};
                int al = ecrt_master_get_slave(master, (uint16_t)i, &slave) == 0 && topology_ok
                    ? slave.al_state | (slave.error_flag ? 0x10 : 0) : -1;
                printf("ecat\tal\t%u\t%d\t-1\n", i, al);
            }
        }
        int valid = topology_ok && state.wc_state == EC_WC_COMPLETE;
        for (unsigned i = 0; valid && i < count; ++i)
        {
            ec_slave_config_state_t config = {0};
            ecrt_slave_config_state(configs[i], &config);
            valid = config.online && config.operational;
        }
        if (!valid)
            image_clear_outputs(image);
        if (assign)
        {
            ecrt_master_application_time(master, monotonic_ns());
            ecrt_master_sync_reference_clock(master);
            ecrt_master_sync_slave_clocks(master);
        }
        ecrt_domain_queue(domain);
        ecrt_master_send(master);
        image_report(image, valid, (int)state.working_counter, -1);
    }
    ecrt_master_receive(master);
    image_clear_outputs(image);
    ecrt_domain_queue(domain);
    ecrt_master_send(master);
    result = 0;
    goto done;
failed:
    pdo_error("IgH could not create or verify an online PDO domain for the scanned topology.");
done:
    free(configs);
    free(image);
    return result;
}

#endif
