#include "wrapper.h"
#include <stdio.h>

struct nm_desc *net_nm_open(const char *ifname, const struct nmreq *req,
                            uint64_t new_flags, const struct nm_desc *arg)
{
    // nm_open — это static inline функция из netmap_user.h, она будет встроена сюда
    struct nm_desc *d = nm_open(ifname, req, new_flags, arg);
    if (d == NULL)
    {
        perror("my_nm_open: nm_open failed");
    }
    return d;
}

void net_nm_close(struct nm_desc *d)
{
    nm_close(d);
}