/*
 * Copyright (c) 2013 Grzegorz Kostka (kostka.grzegorz@gmail.com)
 * All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted provided that the following conditions
 * are met:
 *
 * - Redistributions of source code must retain the above copyright
 *   notice, this list of conditions and the following disclaimer.
 * - Redistributions in binary form must reproduce the above copyright
 *   notice, this list of conditions and the following disclaimer in the
 *   documentation and/or other materials provided with the distribution.
 * - The name of the author may not be used to endorse or promote products
 *   derived from this software without specific prior written permission.
 *
 * THIS SOFTWARE IS PROVIDED BY THE AUTHOR ``AS IS'' AND ANY EXPRESS OR
 * IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES
 * OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED.
 * IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY DIRECT, INDIRECT,
 * INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT
 * NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
 * DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
 * THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
 * (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF
 * THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
 */

/** @addtogroup lwext4
 * @{
 */
/**
 * @file  ext4_bcache.c
 * @brief Block cache allocator.
 */

#include <ext4_config.h>
#include <ext4_types.h>
#include <ext4_bcache.h>
#include <ext4_blockdev.h>
#include <ext4_debug.h>
#include <ext4_errno.h>
#include <ext4_fs.h>

#include <string.h>
#include <stdlib.h>

static int ext4_bcache_lba_compare(struct ext4_buf *a, struct ext4_buf *b)
{
	 if (a->lba > b->lba)
		 return 1;
	 else if (a->lba < b->lba)
		 return -1;
	 return 0;
}

static int ext4_bcache_lru_compare(struct ext4_buf *a, struct ext4_buf *b)
{
	if (a->lru_id > b->lru_id)
		return 1;
	else if (a->lru_id < b->lru_id)
		return -1;
	return 0;
}

RB_GENERATE_INTERNAL(ext4_buf_lba, ext4_buf, lba_node,
		     ext4_bcache_lba_compare, static inline)
RB_GENERATE_INTERNAL(ext4_buf_lru, ext4_buf, lru_node,
		     ext4_bcache_lru_compare, static inline)

/* Generic lwext4 builds may run without a scheduler integration. Kairix
 * provides a strong definition which yields the current task. */
__attribute__((weak)) void ext4_bcache_yield(void)
{
}

void ext4_bcache_lock(struct ext4_bcache *bc)
{
	uintptr_t owner = ext4_lock_owner();
	uint32_t ticket = __atomic_fetch_add(&bc->state_next_ticket, 1,
					     __ATOMIC_RELAXED);
	bool contended =
		ticket != __atomic_load_n(&bc->state_serving_ticket,
					  __ATOMIC_ACQUIRE);
	while (__atomic_load_n(&bc->state_serving_ticket, __ATOMIC_ACQUIRE) !=
	       ticket) {
		ext4_lock_progress(3, 1,
				   __atomic_load_n(&bc->state_owner,
						   __ATOMIC_RELAXED),
				   __atomic_load_n(&bc->state_contentions,
						   __ATOMIC_RELAXED));
		ext4_bcache_yield();
	}
	__atomic_store_n(&bc->state_owner, owner, __ATOMIC_RELAXED);
	__atomic_store_n(&bc->state_lock, 1, __ATOMIC_RELEASE);
	if (contended) {
		__atomic_add_fetch(&bc->state_contentions, 1, __ATOMIC_RELAXED);
		/* Clear the wait marker once this caller owns the bookkeeping
		 * lock. Avoid a global diagnostic atomic on every uncontended cache
		 * operation. */
		ext4_lock_progress(3, 0, 0,
				   __atomic_load_n(&bc->state_contentions,
						   __ATOMIC_RELAXED));
	}
}

void ext4_bcache_unlock(struct ext4_bcache *bc)
{
	uintptr_t owner = ext4_lock_owner();
	ext4_assert(__atomic_load_n(&bc->state_lock, __ATOMIC_RELAXED));
	ext4_assert(__atomic_load_n(&bc->state_owner, __ATOMIC_RELAXED) == owner);
	__atomic_store_n(&bc->state_owner, 0, __ATOMIC_RELAXED);
	__atomic_store_n(&bc->state_lock, 0, __ATOMIC_RELAXED);
	__atomic_add_fetch(&bc->state_serving_ticket, 1, __ATOMIC_RELEASE);
}

int ext4_bcache_init_dynamic(struct ext4_bcache *bc, uint32_t cnt,
			     uint32_t itemsize)
{
	ext4_assert(bc && cnt && itemsize);

	memset(bc, 0, sizeof(struct ext4_bcache));

	bc->cnt = cnt;
	bc->itemsize = itemsize;
	bc->ref_blocks = 0;
	bc->max_ref_blocks = 0;

	return EOK;
}

void ext4_bcache_cleanup(struct ext4_bcache *bc)
{
	struct ext4_buf *buf, *tmp;
	RB_FOREACH_SAFE(buf, ext4_buf_lba, &bc->lba_root, tmp) {
		ext4_block_flush_buf(bc->bdev, buf);
		ext4_bcache_drop_buf(bc, buf);
	}
}

int ext4_bcache_fini_dynamic(struct ext4_bcache *bc)
{
	memset(bc, 0, sizeof(struct ext4_bcache));
	return EOK;
}

/**@brief:
 *
 *  This is ext4_bcache, the module handling basic buffer-cache stuff.
 *
 *  Buffers in a bcache are sorted by their LBA and stored in a
 *  RB-Tree(lba_root).
 *
 *  Bcache also maintains another RB-Tree(lru_root) right now, where
 *  buffers are sorted by their LRU id.
 *
 *  A singly-linked list is used to track those dirty buffers which are
 *  ready to be flushed. (Those buffers which are dirty but also referenced
 *  are not considered ready to be flushed.)
 *
 *  When a buffer is not referenced, it will be stored in both lba_root
 *  and lru_root, while it will only be stored in lba_root when it is
 *  referenced.
 */

static struct ext4_buf *
ext4_buf_alloc(struct ext4_bcache *bc, uint64_t lba)
{
	void *data;
	struct ext4_buf *buf;
	data = ext4_malloc(bc->itemsize);
	if (!data)
		return NULL;

	buf = ext4_calloc(1, sizeof(struct ext4_buf));
	if (!buf) {
		ext4_free(data);
		return NULL;
	}

	buf->lba = lba;
	buf->data = data;
	buf->bc = bc;
	return buf;
}

static void ext4_buf_free(struct ext4_buf *buf)
{
	ext4_free(buf->data);
	ext4_free(buf);
}

static struct ext4_buf *
ext4_buf_lookup(struct ext4_bcache *bc, uint64_t lba)
{
	struct ext4_buf tmp = {
		.lba = lba
	};

	return RB_FIND(ext4_buf_lba, &bc->lba_root, &tmp);
}

static struct ext4_buf *
ext4_bcache_find_get_locked(struct ext4_bcache *bc, struct ext4_block *b,
			    uint64_t lba)
{
	struct ext4_buf *buf = ext4_buf_lookup(bc, lba);
	if (buf) {
		/* If buffer is not referenced. */
		if (!buf->refctr) {
			/* Assign new value to LRU id and increment LRU counter. */
			buf->lru_id = ++bc->lru_ctr;
			RB_REMOVE(ext4_buf_lru, &bc->lru_root, buf);
			if (ext4_bcache_test_flag(buf, BC_DIRTY))
				ext4_bcache_remove_dirty_node(bc, buf);
		}

		ext4_bcache_inc_ref(buf);
		b->lb_id = lba;
		b->buf = buf;
		b->data = buf->data;
	}
	return buf;
}

static void ext4_bcache_drop_buf_locked(struct ext4_bcache *bc,
					struct ext4_buf *buf)
{
	/* Warn on dropping any referenced buffers.*/
	if (buf->refctr) {
		ext4_dbg(DEBUG_BCACHE, DBG_WARN "Buffer is still referenced. "
			 "lba: %" PRIu64 ", refctr: %" PRIu32 "\n",
			 buf->lba, buf->refctr);
	} else
		RB_REMOVE(ext4_buf_lru, &bc->lru_root, buf);

	RB_REMOVE(ext4_buf_lba, &bc->lba_root, buf);
	if (ext4_bcache_test_flag(buf, BC_DIRTY))
		ext4_bcache_remove_dirty_node(bc, buf);

	ext4_buf_free(buf);
	bc->ref_blocks--;
}

int ext4_bcache_shake_prepare(struct ext4_bcache *bc,
			     struct ext4_buf **dirty_buf)
{
	*dirty_buf = NULL;
	ext4_bcache_lock(bc);
	if (RB_EMPTY(&bc->lru_root) || bc->cnt > bc->ref_blocks) {
		ext4_bcache_unlock(bc);
		return 0;
	}
	struct ext4_buf *buf = RB_MIN(ext4_buf_lru, &bc->lru_root);
	ext4_assert(buf && !buf->refctr);
	if (ext4_bcache_test_flag(buf, BC_DIRTY)) {
		RB_REMOVE(ext4_buf_lru, &bc->lru_root, buf);
		ext4_bcache_remove_dirty_node(bc, buf);
		ext4_bcache_inc_ref(buf);
		*dirty_buf = buf;
		ext4_bcache_unlock(bc);
		return 2;
	} else {
		ext4_bcache_drop_buf_locked(bc, buf);
	}
	ext4_bcache_unlock(bc);
	return 1;
}

struct ext4_buf *ext4_buf_lowest_lru(struct ext4_bcache *bc)
{
	return RB_MIN(ext4_buf_lru, &bc->lru_root);
}

void ext4_bcache_drop_buf(struct ext4_bcache *bc, struct ext4_buf *buf)
{
	ext4_bcache_lock(bc);
	ext4_bcache_drop_buf_locked(bc, buf);
	ext4_bcache_unlock(bc);
}

void ext4_bcache_invalidate_buf(struct ext4_bcache *bc,
				struct ext4_buf *buf)
{
	buf->end_write = NULL;
	buf->end_write_arg = NULL;

	/* Clear both dirty and up-to-date flags. */
	if (ext4_bcache_test_flag(buf, BC_DIRTY))
		ext4_bcache_remove_dirty_node(bc, buf);

	ext4_bcache_clear_dirty(buf);
}

void ext4_bcache_invalidate_lba(struct ext4_bcache *bc,
				uint64_t from,
				uint32_t cnt)
{
	ext4_bcache_lock(bc);
	uint64_t end = from + cnt - 1;
	struct ext4_buf *tmp = ext4_buf_lookup(bc, from), *buf;
	RB_FOREACH_FROM(buf, ext4_buf_lba, tmp) {
		if (buf->lba > end)
			break;

		ext4_bcache_invalidate_buf(bc, buf);
	}
	ext4_bcache_unlock(bc);
}

struct ext4_buf *
ext4_bcache_find_get(struct ext4_bcache *bc, struct ext4_block *b,
		     uint64_t lba)
{
	ext4_bcache_lock(bc);
	struct ext4_buf *buf = ext4_bcache_find_get_locked(bc, b, lba);
	ext4_bcache_unlock(bc);
	return buf;
}

int ext4_bcache_alloc(struct ext4_bcache *bc, struct ext4_block *b,
		      bool *is_new)
{
	/* Try to search the buffer with exaxt LBA. */
	ext4_bcache_lock(bc);
	struct ext4_buf *buf = ext4_bcache_find_get_locked(bc, b, b->lb_id);
	if (buf) {
		*is_new = false;
		ext4_bcache_unlock(bc);
		return EOK;
	}
	ext4_bcache_unlock(bc);

	/* Allocate outside the bookkeeping lock, then publish with a second
	 * lookup so concurrent misses create only one cache entry. */
	buf = ext4_buf_alloc(bc, b->lb_id);
	if (!buf)
		return ENOMEM;
	ext4_bcache_lock(bc);
	struct ext4_buf *raced = ext4_bcache_find_get_locked(bc, b, b->lb_id);
	if (raced) {
		*is_new = false;
		ext4_bcache_unlock(bc);
		ext4_buf_free(buf);
		return EOK;
	}

	RB_INSERT(ext4_buf_lba, &bc->lba_root, buf);
	/* One more buffer in bcache now. :-) */
	bc->ref_blocks++;

	/*Calc ref blocks max depth*/
	if (bc->max_ref_blocks < bc->ref_blocks)
		bc->max_ref_blocks = bc->ref_blocks;


	ext4_bcache_inc_ref(buf);
	/* Assign new value to LRU id and increment LRU counter
	 * by 1*/
	buf->lru_id = ++bc->lru_ctr;

	b->buf = buf;
	b->data = buf->data;

	*is_new = true;
	ext4_bcache_unlock(bc);
	return EOK;
}

int ext4_bcache_free(struct ext4_bcache *bc, struct ext4_block *b)
{
	struct ext4_buf *buf = b->buf;
	bool flush_now = false;
	bool drop_after_flush = false;
	bool internal_pin = false;

	ext4_assert(bc && b);

	/*Check if valid.*/
	ext4_assert(b->lb_id);

	/*Block should have a valid pointer to ext4_buf.*/
	ext4_assert(buf);

	ext4_bcache_lock(bc);
	/*Check if someone don't try free unreferenced block cache.*/
	ext4_assert(buf->refctr);
	/*Just decrease reference counter*/
	ext4_bcache_dec_ref(buf);

	/* We are the last external reference touching this buffer. Decide the
	 * cleanup while state_lock still serializes refcount/LRU membership. */
	if (!buf->refctr) {
		/* This buffer is ready to be flushed. */
		if (ext4_bcache_test_flag(buf, BC_DIRTY) &&
		    ext4_bcache_test_flag(buf, BC_UPTODATE)) {
			if (bc->bdev->cache_write_back &&
			    !ext4_bcache_test_flag(buf, BC_FLUSH) &&
			    !ext4_bcache_test_flag(buf, BC_TMP))
				ext4_bcache_insert_dirty_node(bc, buf);
			else
				flush_now = true;
		}

		/* The buffer is invalidated...drop it. */
		if (!ext4_bcache_test_flag(buf, BC_UPTODATE) ||
		    ext4_bcache_test_flag(buf, BC_TMP))
			drop_after_flush = true;

		if (flush_now || drop_after_flush) {
			/* Immediate cleanup runs after dropping state_lock. Keep an
			 * internal reference and leave the buffer out of lru_root so a
			 * concurrent cache shake cannot free it underneath the flush. */
			ext4_bcache_inc_ref(buf);
			internal_pin = true;
		} else {
			RB_INSERT(ext4_buf_lru, &bc->lru_root, buf);
		}
	}
	ext4_bcache_unlock(bc);

	if (flush_now) {
		ext4_block_flush_buf(bc->bdev, buf);
	}
	if (internal_pin) {
		ext4_bcache_lock(bc);
		ext4_assert(buf->refctr);
		if (flush_now)
			ext4_bcache_clear_flag(buf, BC_FLUSH);
		ext4_bcache_dec_ref(buf);
		if (!buf->refctr) {
			/* ext4_bcache_drop_buf_locked() expects every unreferenced
			 * buffer to be present in lru_root. Restore that invariant
			 * before either retaining or dropping the buffer. */
			RB_INSERT(ext4_buf_lru, &bc->lru_root, buf);
			if (drop_after_flush)
				ext4_bcache_drop_buf_locked(bc, buf);
			else if (ext4_bcache_test_flag(buf, BC_DIRTY))
				ext4_bcache_insert_dirty_node(bc, buf);
		}
		ext4_bcache_unlock(bc);
	}

	b->lb_id = 0;
	b->data = 0;

	return EOK;
}

bool ext4_bcache_is_full(struct ext4_bcache *bc)
{
	ext4_bcache_lock(bc);
	bool full = bc->cnt <= bc->ref_blocks;
	ext4_bcache_unlock(bc);
	return full;
}


/**
 * @}
 */
