import io
import os

from setuptools import find_packages, setup

__version__ = "0.1.0"


def read_from(file):
    reply = []
    with io.open(os.path.join(here, file), encoding='utf8') as f:
        for l in f:
            l = l.strip()
            if not l:
                break
            if l[:2] == '-r':
                reply += read_from(l.split(' ')[1])
                continue
            if l[0] != '#' or l[:2] != '//':
                reply.append(l)
    return reply


here = os.path.abspath(os.path.dirname(__file__))
with io.open(os.path.join(here, 'README.rst'), encoding='utf8') as f:
    README = f.read()
#with io.open(os.path.join(here, 'CHANGELOG.md'), encoding='utf8') as f:
#    CHANGES = f.read()

setup(
    name="test_chan",
    version=__version__,
    packages=find_packages(),
    description='ChannelServer integration tester',
    long_description=README,
    classifiers=[
        "Topic :: Internet :: WWW/HTTP",
        'Programming Language :: Python',
        "Programming Language :: Python :: 2",
        "Programming Language :: Python :: 2.7",
        "Programming Language :: Python :: 3",
        "Programming Language :: Python :: 3.4",
        "Programming Language :: Python :: 3.5",
        "Programming Language :: Python :: 3.6",
    ],
    keywords='test',
    author="JR Conlin",
    author_email="src+pairsona@jrconlin.com",
    url='https://github.com/mozilla-services/channelserver/tree/master/test_chan',
    license="MPL2",
    test_suite="nose.collector",
    include_package_data=True,
    zip_safe=False,
    install_requires=read_from('requirements.txt'),
    entry_points="""
    [console_scripts]
    tester = test_chan.__main__:main
    """,
)
