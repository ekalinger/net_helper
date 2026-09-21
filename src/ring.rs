//#![cfg(feature = "sys")]

use libc::{POLLOUT, c_ulong, ioctl, poll, pollfd};
use std::io;
use std::marker::PhantomData;
use std::os::fd::AsRawFd;
use std::ptr;
use std::slice;

use crate::Netmap;
use crate::error::Error;
use crate::ffi;
use crate::frame::Frame;

const NIOCTXSYNC: libc::c_ulong = 0x00006994;

/// A Netmap ring (tx/rx)
pub struct Ring<'a> {
    ring: *mut ffi::netmap_ring,
    index: usize,
    _marker: PhantomData<&'a mut ffi::netmap_ring>,
}

unsafe impl<'a> Send for Ring<'a> {}

/// A TX ring
pub struct TxRing<'a>(Ring<'a>);

/// An RX ring
pub struct RxRing<'a>(Ring<'a>);

impl<'a> Ring<'a> {
    /// Create a new ring
    pub(crate) fn new(ring: *mut ffi::netmap_ring, index: usize) -> Self {
        Self {
            ring,
            index,
            _marker: PhantomData,
        }
    }

    /// Get the ring index (the ID of this ring).
    pub fn index(&self) -> usize {
        self.index
    }

    /// Get the total number of slots in this ring.
    pub fn num_slots(&self) -> usize {
        unsafe { (*self.ring).num_slots as usize }
    }
}

impl<'a> TxRing<'a> {
    /// create a new tx ring
    pub(crate) fn new(ring: *mut ffi::netmap_ring, index: usize) -> Self {
        Self(Ring::new(ring, index))
    }

    /// Отправка одного пакета (TX)
    pub fn send(&mut self, buf: &[u8]) -> Result<(), Error> {
        if buf.len() > self.max_payload_size() {
            return Err(Error::PacketTooLarge(buf.len()));
        }

        unsafe {
            let ring = self.0.ring;

            let cur = (*ring).cur;
            let tail = (*ring).tail;
            let num_slots = (*ring).num_slots;

            // 1. Проверяем, есть ли свободное место в кольце для отправки
            if cur == tail {
                // Кольцо заполнено. Нужно вызвать ioctl(..., NIOCTXSYNC) или poll(),
                // чтобы ядро обновило указатель tail, освободив отправленные слоты.
                return Err(Error::InsufficientSpace);
            }

            // 2. Получаем указатель на текущий слот
            // В netmap_ring массив slot является "гибким массивом" (flexible array member).
            // Безопасное получение указателя на элемент массива slot[cur]:
            let slot_ptr = (*ring).slot.as_ptr().add(cur as usize) as *mut ffi::netmap_slot;

            // 3. ПРАВИЛЬНЫЙ расчет адреса буфера (аналог макроса NETMAP_BUF)
            let ring_ptr = ring as *mut u8;
            let buf_ofs = (*ring).buf_ofs as usize;
            let buf_size = (*ring).nr_buf_size as usize; // используем динамический размер вместо 2048
            let buf_idx = (*slot_ptr).buf_idx as usize;

            let buf_ptr = ring_ptr.add(buf_ofs).add(buf_idx * buf_size);

            // 4. Копируем данные пакета в буфер netmap
            ptr::copy_nonoverlapping(buf.as_ptr(), buf_ptr, buf.len());
            (*slot_ptr).len = buf.len() as u16;

            // 5. Продвигаем указатели cur и head с учетом циклического размера кольца (модуль num_slots)
            // Для netmap критически важно, чтобы head и cur двигались синхронно при поштучной отправке
            let next_index = if cur + 1 >= num_slots { 0 } else { cur + 1 };

            (*ring).cur = next_index;
            (*ring).head = next_index;

            // Внимание: Пакет не уйдет в сеть физически, пока вы не вызовете
            // ioctl(fd, NIOCTXSYNC) или poll() на дескрипторе netmap!
            Ok(())
        }
    }

    pub fn sync(&self, nm: &Netmap) {
        unsafe {
            let mut pfd = pollfd {
                fd: nm.as_raw_fd(), // Файловый дескриптор /dev/netmap
                events: POLLOUT,    // Ждем готовности к отправке (это стриггерит sync в ядре)
                revents: 0,
            };

            // Таймаут 10-100 мс (не передавайте 0, дайте ядру время обработать пакеты)
            let ret = poll(&mut pfd, 1, 10);

            if ret < 0 {
                eprintln!("Ошибка poll: {}", io::Error::last_os_error());
            }
        }
    }

    pub fn sync_ioctl(&self, nm: &Netmap) {
        unsafe {
            // Третий аргумент передаем как 0 (или NULL), так как NIOCTXSYNC не требует параметров
            let ret = libc::ioctl(nm.as_raw_fd(), NIOCTXSYNC, 0);

            if ret < 0 {
                let err = io::Error::last_os_error();
                // Игнорируем EINTR (вызов прерван сигналом), остальные ошибки логируем
                if err.kind() != io::ErrorKind::Interrupted {
                    eprintln!("Ошибка ioctl NIOCTXSYNC: {}", err);
                }
            }
        }
    }

    /// get the maximum payload size for this ring
    pub fn max_payload_size(&self) -> usize {
        unsafe { (*self.0.ring).nr_buf_size as usize }
    }

    /// reserve space for batch sending
    pub fn reserve_batch(&mut self, count: usize) -> Result<BatchReservation<'a>, Error> {
        unsafe {
            let ring_ptr = self.0.ring;
            let head = (*ring_ptr).head;
            let tail = (*ring_ptr).tail;
            let num_slots = (*ring_ptr).num_slots as u32;

            // Calculate available space. Netmap rings are full when head == tail + 1 (modulo num_slots)
            // So, available space is num_slots - 1 - current_used_slots
            // current_used_slots = (head - tail + num_slots) % num_slots
            let current_used_slots = (head.wrapping_sub(tail).wrapping_add(num_slots)) % num_slots;
            let available_slots = (num_slots - 1).saturating_sub(current_used_slots) as usize;

            if available_slots < count {
                return Err(Error::InsufficientSpace);
            }
        }

        Ok(BatchReservation {
            ring: self.0.ring,
            start: unsafe { (*self.0.ring).head },
            count,
            _marker: PhantomData,
        })
    }

    /// Зарезервировать все доступные слоты в кольце на данный момент.
    /// Возвращает объект батча и количество успешно зарезервированных слотов.
    pub fn reserve_all(&mut self) -> Result<(BatchReservation<'a>, usize), Error> {
        unsafe {
            let ring_ptr = self.0.ring;
            let head = (*ring_ptr).head;
            let tail = (*ring_ptr).tail;
            let num_slots = (*ring_ptr).num_slots; // Поле имеет тип u32

            // 1. Вычисляем текущее количество занятых слотов
            let current_used_slots = (head.wrapping_sub(tail).wrapping_add(num_slots)) % num_slots;

            // 2. Вычисляем максимально доступное пространство (максимум num_slots - 1)
            let available_slots = (num_slots - 1).saturating_sub(current_used_slots) as usize;

            // Если свободных слотов вообще нет, возвращаем ошибку
            if available_slots == 0 {
                return Err(Error::InsufficientSpace);
            }

            // 3. Резервируем все доступные слоты
            let reservation = BatchReservation {
                ring: self.0.ring,
                start: head,
                count: available_slots,
                _marker: PhantomData,
            };

            Ok((reservation, available_slots))
        }
    }
}

/// a batch reservation for tx packets
pub struct BatchReservation<'a> {
    ring: *mut ffi::netmap_ring,
    start: u32,
    count: usize,
    _marker: PhantomData<&'a mut ffi::netmap_ring>,
}

impl<'a> BatchReservation<'a> {
    /// get a mutable slice for packet in the batch
    pub fn packet(&mut self, index: usize, len: usize) -> Result<&mut [u8], Error> {
        if index >= self.count {
            return Err(Error::InvalidRingIndex(index));
        }
        unsafe {
            let num_slots = (*self.ring).num_slots;
            let slot_idx = (self.start + index as u32) % num_slots;
            let slot = (*self.ring).slot.as_ptr().add(slot_idx as usize) as *mut ffi::netmap_slot;

            (*slot).len = len as u16;

            // ПРАВИЛЬНЫЙ расчет виртуального адреса буфера (аналогично вашему методе send)
            let ring_ptr = self.ring as *mut u8;
            let buf_ofs = (*self.ring).buf_ofs as usize;
            let buf_size = (*self.ring).nr_buf_size as usize;
            let buf_idx = (*slot).buf_idx as usize;

            let buf_ptr = ring_ptr.add(buf_ofs).add(buf_idx * buf_size);

            Ok(slice::from_raw_parts_mut(buf_ptr, len))
        }
    }

    /// commit the batch (make packets visible to NIC)
    pub fn commit(self, written: usize) {
        // Защитная проверка: нельзя закомитить больше, чем зарезервировали
        let actual_written = written.min(self.count);

        if actual_written == 0 {
            // Если ничего не записали, просто выходим.
            // Указатели head и cur в кольце не изменятся, слоты останутся свободными.
            return;
        }

        unsafe {
            let num_slots = (*self.ring).num_slots;

            // Сдвигаем head и cur ТОЛЬКО на количество реально записанных пакетов
            let new_head = (self.start + actual_written as u32) % num_slots;

            (*self.ring).cur = new_head;
            (*self.ring).head = new_head;
        }
    }
}

impl<'a> RxRing<'a> {
    /// create a new rx ring
    pub(crate) fn new(ring: *mut ffi::netmap_ring, index: usize) -> Self {
        Self(Ring::new(ring, index))
    }

    /// receive single packet
    pub fn recv(&mut self) -> Option<Frame<'_>> {
        unsafe {
            let ring = self.0.ring; // *mut netmap_ring
            println!(
                "DEBUG: head={}, cur={}, tail={}",
                (*ring).head,
                (*ring).cur,
                (*ring).tail
            );
            // 1. Проверяем, есть ли новые данные
            if (*ring).head == (*ring).tail {
                return None;
            }

            // 2. Безопасно получаем слот по индексу head через сырой указатель
            let head_idx = (*ring).head as usize;
            let slot_ptr = (*ring).slot.as_ptr().add(head_idx);
            let slot = &*slot_ptr; // Теперь это &netmap_slot

            let len = slot.len as usize;
            let buf_idx = slot.buf_idx as usize;

            // 3. Вычисляем адрес буфера (NETMAP_BUF)
            let ring_bytes = ring as *const u8;
            let buf_base = ring_bytes.offset((*ring).buf_ofs as isize);
            let raw_ptr = buf_base.add(buf_idx * (*ring).nr_buf_size as usize);

            // Создаем срез данных
            let buf = if raw_ptr.is_null() || len == 0 {
                &[]
            } else {
                std::slice::from_raw_parts(raw_ptr, len)
            };

            // 4. Продвигаем head и cur вперед по кольцу
            let next_idx = if head_idx + 1 >= (*ring).num_slots as usize {
                0
            } else {
                head_idx + 1
            } as u32;

            (*ring).head = next_idx;
            (*ring).cur = next_idx;

            Some(Frame::new(buf))
        }
    }

    /// Sync rx queue
    pub fn sync(&self, nm: &Netmap) {
        // Использование правильной константы для синхронизации RX
        const NIOCRXSYNC: c_ulong = 0x6995;
        unsafe {
            let result = ioctl(nm.as_raw_fd(), NIOCRXSYNC);
            if result < 0 {
                eprintln!("Ошибка RXSYNC ioctl: {}", io::Error::last_os_error());
            }
        }
    }

    /// receive a batch of packets
    /// Возвращает количество реально прочитанных пакетов записанных в `batch`
    pub fn recv_batch(&mut self, batch: &mut [Frame<'_>]) -> usize {
        unsafe {
            let ring = self.0.ring;
            let head = (*ring).head;
            let tail = (*ring).tail;
            let num_slots = (*ring).num_slots;

            // 1. Вычисляем сколько слотов доступно для чтения на данный момент.
            // В RX кольце доступные пакеты находятся между head и tail.
            let avail = if head <= tail {
                (tail - head) as usize
            } else {
                (num_slots - head + tail) as usize
            };

            // Берем минимум между доступными пакетами и размером переданного буфера
            let count = avail.min(batch.len());
            if count == 0 {
                return 0;
            }

            let ring_bytes = ring as *const u8;
            let buf_base = ring_bytes.add((*ring).buf_ofs as usize);
            let buf_size = (*ring).nr_buf_size as usize;

            // 2. Извлекаем пакеты из кольца
            for i in 0..count {
                // Индексы идут последовательно от текущего head с учетом зацикленности кольца
                let slot_idx = (head + i as u32) % num_slots;
                let slot_ptr = (*ring).slot.as_ptr().add(slot_idx as usize);

                let len = (*slot_ptr).len as usize;
                let buf_idx = (*slot_ptr).buf_idx as usize;

                // Вычисление правильного виртуального адреса
                let raw_ptr = buf_base.add(buf_idx * buf_size);

                let buf = if raw_ptr.is_null() || len == 0 {
                    &[]
                } else {
                    slice::from_raw_parts(raw_ptr, len)
                };

                batch[i] = Frame::new(buf);
            }

            // 3. Продвигаем указатели head и cur на количество прочитанных пакетов
            let new_head = (head + count as u32) % num_slots;
            (*ring).head = new_head;
            (*ring).cur = new_head;

            count
        }
    }
}
