DOCKER_TAG ?= kairix:latest
.PHONY: docker build_docker
	
docker:
	docker run --rm -it -v ${PWD}:/mnt -w /mnt --name kairix ${DOCKER_TAG} bash

build_docker: 
	docker build -t ${DOCKER_TAG} --target build .

fmt:
	cd easy-fs; cargo fmt; cd ../easy-fs-fuse; cargo fmt; cd ../os ; cargo fmt; cd ../user; cargo fmt; cd ..
