use net_helper::*;
use std::arch::x86_64::{
    __m512i, _mm_sfence, _mm512_add_epi32, _mm512_cmpeq_epi32_mask, _mm512_mask_blend_epi32,
    _mm512_mask_sub_epi32, _mm512_set_epi32, _mm512_set1_epi8, _mm512_stream_si512,
    _mm512_sub_epi32,
};
use std::env;

fn main() -> Result<(), Error> {
    //let test_packet: [u8; 64] = [
    //    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, // 1. Dest MAC: Broadcast
    //    0x00, 0x0a, 0xcd, 0x11, 0x22, 0x33, // 2. Src MAC: Любой фейковый
    //    0x12,
    //    0x34, // 3. EtherType: Кастомный (НЕ 0x0800!), чтобы обойти проверку IP
    //    // Остальные 46 байт - просто тестовый мусор (payload)
    //    0xaa, 0xbb, 0xcc, 0xdd, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a,
    //    0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19,
    //    0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x22,
    //    0x23, 0x24, 0x25, 0x26, 0x27,
    //];

    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        panic!("Нет интерфейса")
    }

    const PACKET_COUNT: usize = 100_000;

    // Исходные MAC-адреса (взяты из вашего дампа)
    let dst_mac: u64 = 0xDDCCBBAA2211;
    let src_mac: u64 = 0xFFEE66554433;

    // Формируем элементы Ethernet-заголовка для x86 (с учетом Little Endian)
    let elem_0 = ((dst_mac >> 16) & 0xFFFFFFFF) as i32;
    let elem_1 = (((dst_mac & 0xFFFF) << 16) | ((src_mac >> 32) & 0xFFFF)) as i32;
    let elem_2 = (src_mac & 0xFFFFFFFF) as i32;

    // Элемент #3: Старшие 16 бит — IP заголовок (0x4500), Младшие — EtherType IPv4 (0x0800)
    let elem_3 = 0x45000800i32;

    // Элемент #4: Total Length (46 байт = 0x002E) + IP ID (0x0001)
    let elem_4 = 0x0001002Ei32;

    // Элемент #5: Flags/Frag (0x4000) + TTL (64 = 0x40) + Protocol (UDP = 17 = 0x11)
    let elem_5 = 0x40114000i32;

    // Элемент #6: Начальная IP Checksum (0x0005) + Старшие 2 байта Src IP (192.168 -> C0 A8)
    let elem_6 = 0xC0A80005u32 as i32;

    // Элемент #7: Младшие 2 байта Src IP (1.1 -> 01 01) + Старшие 2 байта Dst IP (192.168 -> C0 A8)
    let elem_7 = 0xC0A80101u32 as i32;

    // Элемент #8: Младшие 2 байта Dst IP (.1.10 -> 01 0A) + UDP Src Port (80 -> 0x0050)
    let base_dst_ip_end = 0x010A;
    let base_src_port = 0x0050;
    let elem_8 = ((base_src_port << 16) | base_dst_ip_end) as i32;

    // Элемент #9: UDP Dst Port (8080 -> 0x1F90) + UDP Length (26 байт = 0x001A)
    let base_dst_port = 0x1F90;
    let udp_len = 0x001A;
    let elem_9 = ((udp_len << 16) | base_dst_port) as i32;

    // Open the netmap interface
    let nm = NetmapBuilder::new(&args[1])
        //.num_tx_rings(1)
        //.num_rx_rings(1)
        .build()?;
    println!("{}", nm.num_rx_rings());
    let mut tx_ring = nm.tx_ring(0)?;

    //let mut rx_ring = nm.rx_ring(1)?;

    unsafe {
        if is_x86_feature_detected!("avx512f") {
            // Загружаем ZMM. Порядок аргументов обратный: снизу вверх от 15 до 0.
            let template_zmm = _mm512_set_epi32(
                0, 0, 0, 0, 0, 0, elem_9, elem_8, elem_7, elem_6, elem_5, elem_4, elem_3, elem_2,
                elem_1, elem_0,
            );

            let mut current_packet_zmm = template_zmm;

            // Вектор инкремента для динамических полей (Dst Port +1, Dst IP +1)
            let increment_zmm = _mm512_set_epi32(
                0, 0, 0, 0, 0, 0, 1, // Элемент #9
                1, // Элемент #8
                0, 0, 0, 0, 0, 0, 0, 0,
            );

            // Вектор декремента контрольной суммы (Элемент #6, старшие 16 бит)
            let checksum_decrement_zmm = _mm512_set_epi32(
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                1 << 16, // Элемент #6 (Checksum -1)
                0,
                0,
                0,
                0,
                0,
                0,
            );

            // Ожидаемый триггер обнуления чексуммы (0x0000C0A8)
            let checksum_zero_trigger = _mm512_set_epi32(
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0x0000C0A8i32, // Элемент #6
                0,
                0,
                0,
                0,
                0,
                0,
            );

            // Коррекция: вычитаем 1 << 16 из занулившейся чексуммы, чтобы получить 0xFFFF
            let checksum_correction_zmm = _mm512_set_epi32(
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                1 << 16, // Элемент #6
                0,
                0,
                0,
                0,
                0,
                0,
            );

            // Маска смешивания для шаблона пакета.
            // Изменяются элементы #4 (чексумма), #6 (IP), #7 (порты).
            // Бинарный вид: 1101 0000 -> В шестнадцатеричной системе это 0xD0
            let blend_mask: u16 = 0xD0;
            let template_zmm = current_packet_zmm;

            let payload_zmm1 = _mm512_set1_epi8(0xAAu8 as i8); // Весь регистр забит байтом 0xAA
            let payload_zmm2 = _mm512_set1_epi8(0xBBu8 as i8);
            let payload_zmm3 = _mm512_set1_epi8(0xCCu8 as i8);

            let mut i: usize = 0;
            while i < PACKET_COUNT {
                if let Ok((mut batch, count)) = tx_ring.reserve_all() {
                    let mut packets_written = 0;
                    for z in 0..count {
                        if let Ok(dst_ptr) = batch.packet(z, 64) {
                            //println!("Ok {:?}", &dst_ptr);
                            _mm512_stream_si512(
                                dst_ptr.as_mut_ptr() as *mut __m512i,
                                current_packet_zmm,
                            );

                            _mm512_stream_si512(
                                dst_ptr.as_mut_ptr().add(64) as *mut __m512i,
                                payload_zmm1,
                            );
                            _mm512_stream_si512(
                                dst_ptr.as_mut_ptr().add(128) as *mut __m512i,
                                payload_zmm2,
                            );
                            _mm512_stream_si512(
                                dst_ptr.as_mut_ptr().add(192) as *mut __m512i,
                                payload_zmm3,
                            );

                            let mut next_fields_zmm =
                                _mm512_add_epi32(current_packet_zmm, increment_zmm);
                            next_fields_zmm =
                                _mm512_sub_epi32(next_fields_zmm, checksum_decrement_zmm);

                            let cmp_mask =
                                _mm512_cmpeq_epi32_mask(next_fields_zmm, checksum_zero_trigger);
                            next_fields_zmm = _mm512_mask_sub_epi32(
                                next_fields_zmm,
                                cmp_mask,
                                next_fields_zmm,
                                checksum_correction_zmm,
                            );

                            current_packet_zmm =
                                _mm512_mask_blend_epi32(blend_mask, template_zmm, next_fields_zmm);

                            packets_written += 1;
                        }
                    }
                    _mm_sfence();
                    i += 1;
                    batch.commit(packets_written);

                    // 3. Отправляем в сеть
                    tx_ring.sync_ioctl(&nm);

                    //println!("{}", packets_written);
                }

                i += 1;
            }
        }
    }

    // Send a packet (zero-copy)
    //tx_ring.send(&test_packet)?;
    //tx_ring.sync(&nm);
    //println!("{:?}", test_packet);
    //let mut count = 10;
    //while count > 0 {
    //    rx_ring.sync(&nm);

    // 2. ВЫГРЕБАЕМ ВСЕ ПАКЕТЫ ДО ЕДИНОГО, пока recv() не вернет None
    //    let mut has_packets = false;
    //    while let Some(rx_pack) = rx_ring.recv() {
    //        has_packets = true;
    //        println!(
    //            "Получен пакет, длина payload: {:?}, payload: {:?}",
    //            rx_pack.payload().len(),
    //            rx_pack.payload()
    //        );
    //    }

    // 3. Засыпаем ТОЛЬКО если пакетов вообще не было,
    // и то на очень короткое время, чтобы не переполнять буфер карты
    //    if !has_packets {
    //        std::thread::sleep(std::time::Duration::from_millis(10));
    //    }

    //    count -= 1;
    //}
    Ok(())
}
