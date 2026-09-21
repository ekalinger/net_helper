#ifndef WRAPPER_H
#define WRAPPER_H

#include <net/netmap_user.h>
#include <net/netmap.h>

// Объявляем экспортируемые функции обёртки
struct nm_desc *net_nm_open(const char *ifname, const struct nmreq *req,
                            uint64_t new_flags, const struct nm_desc *arg);
void net_nm_close(struct nm_desc *d);

#endif